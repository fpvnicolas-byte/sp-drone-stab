// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 SP Drone Stab contributors

//! SP Drone Stab "safe mode": the stretches of a clip whose motion data can't be trusted, and the stabilization from the
//! image alone there.
//!
//! On a DJI O4 with the ICM-40609-D gyro, a hard shake makes the attitude track wrong in a way no repair of the track
//! itself fixes: the stabilization then follows motion that never happened and the picture turns to jelly. Gyroflow
//! 1.7.0's "Ignore motion data from the file" measures all of the motion from the video instead, which handles those
//! stretches well, but for the whole clip, and only once the user knows the option is there.
//!
//! Safe mode does it per stretch, by itself:
//! - `find` marks where the motion data is unreliable: the false rotations and the single-axis tremor of the Gyrogate
//!   detector, and dense high-frequency shake on any camera, with a margin around each and close ones merged;
//! - the image is analyzed in those stretches only, as for a file without motion data;
//! - `splice` puts the orientations measured from the image in place of the motion data's there, anchored to the motion
//!   data at both ends of the stretch so the camera path stays continuous. Everywhere else the motion data is used as it
//!   is (after the Gyrogate repair).

use nalgebra::{ UnitQuaternion, Vector3 };
use crate::gyro_source::{ Quat64, TimeQuat };
use crate::gyrogate;

/// Why a stretch is in safe mode, for the plain-language summary in the UI
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Short false rotations of the ICM-40609-D gyro (Gyrogate bursts)
    FalseRotations,
    /// Dense single-axis tremor of the same gyro
    Tremor,
    /// Strong high-frequency shake, whatever the camera
    Shake,
}

/// A stretch of the motion data's timeline (microseconds, the keys of `TimeQuat`) stabilized from the image
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stretch {
    pub start_us: i64,
    pub end_us: i64,
    pub reasons: Vec<Reason>,
}
impl Stretch {
    pub fn duration_s(&self) -> f64 { (self.end_us - self.start_us) as f64 / 1e6 }
}

/// How `find` marks the stretches
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FindSettings {
    /// Added before and after every unreliable sample, seconds: the image analysis needs some frames around the problem
    /// to measure the motion through it, and the splice blends over them
    pub margin_s: f64,
    /// Stretches closer than this are merged, seconds: switching back and forth looks worse than staying in safe mode
    pub merge_gap_s: f64,
    /// RMS of the angular rate above `SHAKE_HIGHPASS_HZ`, over `SHAKE_WINDOW_S`, from which a moment counts as shake,
    /// deg/s. 0 turns the detector off
    pub shake_rms_dps: f64,
    /// How long the shake must keep up, seconds: a stretch is where its average over this long is above
    /// `shake_rms_dps`. Single jolts and the short false rotations of the O4 gyro stay with the motion data (the
    /// Gyrogate repair handles those); a long violent stretch, where the repaired motion still wobbles, goes to the image
    pub sustain_s: f64,
}
impl Default for FindSettings {
    fn default() -> Self {
        // Tuned on the O4 Pro motocross chase (ICM-40609-D, 03/2026): 30 deg/s over 5 s finds its worst stretch, 1:10 to
        // 1:30, as one (it has a calmer 3.5 s in the middle), and the other stretches its pilot found still jumping with
        // the gyro corrections, while the Gyrogate bursts it has all through the calm parts stay with the repaired data
        Self { margin_s: 0.5, merge_gap_s: 4.0, shake_rms_dps: 30.0, sustain_s: 5.0 }
    }
}

/// The angular rate is split at this frequency for the shake detector: real camera moves, even fast flips, are slower
const SHAKE_HIGHPASS_HZ: f64 = 15.0;
const SHAKE_WINDOW_S: f64 = 0.25;

/// The stretches of `data` whose motion data is unreliable, in time order, without overlaps: where it shakes hard for
/// a long time. `gyrogate` are the settings of the DJI O4 detector, `None` where it doesn't apply (another camera, or a
/// unit with the old gyro); its false rotations and tremor inside a stretch are named among its reasons. Empty for
/// clean motion data
pub fn find(data: &TimeQuat, gyrogate: Option<&gyrogate::Settings>, settings: &FindSettings) -> Vec<Stretch> {
    if settings.shake_rms_dps <= 0.0 { return Vec::new(); }
    let mut found = merge(shake_marks(data, settings.shake_rms_dps, settings.sustain_s), data, settings);
    if let Some(gs) = gyrogate {
        for (a, b, reason) in gyrogate_marks(data, gs) {
            for s in found.iter_mut().filter(|s| a <= s.end_us && b >= s.start_us) {
                if !s.reasons.contains(&reason) { s.reasons.push(reason); s.reasons.sort(); }
            }
        }
    }
    found
}

/// The Gyrogate detector's bursts and tremor as time ranges, on the 1 kHz O4 attitude track only
fn gyrogate_marks(data: &TimeQuat, settings: &gyrogate::Settings) -> Vec<(i64, i64, Reason)> {
    let Some((unique, index)) = gyrogate::memory::unique_samples(data) else { return Vec::new(); };
    // The timestamp of each unique sample: that of its first stored entry
    let mut times = vec![i64::MIN; unique.len()];
    for (&t, &i) in data.keys().zip(index.iter()) {
        if times[i] == i64::MIN { times[i] = t; }
    }
    let time = |sample: usize| times[sample.min(times.len() - 1)];
    let rate = gyrogate::UNIQUE_RATE_HZ;
    let mut marks: Vec<(i64, i64, Reason)> = gyrogate::dsp::detect(&unique, rate, settings).iter()
        // Rate sample k lies between unique samples k and k + 1
        .map(|b| (time(b.start_sample), time(b.end_sample + 1), Reason::FalseRotations))
        .collect();
    marks.extend(runs(&gyrogate::dsp::tremor_mask(&unique, rate, settings)).into_iter().map(|(a, b)| (time(a), time(b + 1), Reason::Tremor)));
    marks
}

/// Moments of strong high-frequency shake, from the orientations at whatever rate they come
fn shake_marks(data: &TimeQuat, threshold_dps: f64, sustain_s: f64) -> Vec<(i64, i64, Reason)> {
    let profile = shake_profile(data);
    if profile.len() < 2 { return Vec::new(); }
    let rate_hz = (profile.len() - 1) as f64 / ((profile[profile.len() - 1].0 - profile[0].0) as f64 / 1e6);
    let level: Vec<f64> = profile.iter().map(|p| p.1).collect();
    let sustained = running_mean_scalar(&level, ((sustain_s * rate_hz).round() as usize).max(1));
    let mask: Vec<bool> = sustained.iter().map(|&v| v > threshold_dps).collect();
    // Each sustained run, from its first to its last shaking moment: the averaging spreads it past both
    runs(&mask).into_iter().filter_map(|(a, b)| {
        let first = (a..=b).find(|&i| level[i] > threshold_dps)?;
        let last = (a..=b).rev().find(|&i| level[i] > threshold_dps)?;
        Some((profile[first].0, profile[last].0, Reason::Shake))
    }).collect()
}

/// The shake level over time: the RMS of the angular rate above `SHAKE_HIGHPASS_HZ` over `SHAKE_WINDOW_S`, deg/s,
/// at each distinct sample. Empty when the data is too short or too slow to tell
pub fn shake_profile(data: &TimeQuat) -> Vec<(i64, f64)> {
    let samples: Vec<(i64, &Quat64)> = data.iter().map(|(t, q)| (*t, q)).collect();
    // Body rates between distinct samples (stored duplicates carry no motion)
    let mut rates: Vec<(i64, Vector3<f64>)> = Vec::with_capacity(samples.len());
    let mut prev: Option<(i64, &Quat64)> = None;
    for &(t, q) in &samples {
        if let Some((pt, pq)) = prev {
            if q == pq { continue; }
            let dt = (t - pt) as f64 / 1e6;
            if dt > 0.0 { rates.push((t, (pq.inverse() * q).scaled_axis().map(|v| v.to_degrees() / dt))); }
        }
        prev = Some((t, q));
    }
    if rates.len() < 16 { return Vec::new(); }
    let span_s = (rates[rates.len() - 1].0 - rates[0].0) as f64 / 1e6;
    if span_s <= 0.0 { return Vec::new(); }
    let rate_hz = (rates.len() - 1) as f64 / span_s;
    // Too slow to see the shake: nothing to say
    if rate_hz < 4.0 * SHAKE_HIGHPASS_HZ { return Vec::new(); }

    // High-pass: the rate minus its running mean over one period of the split frequency
    let lp = ((rate_hz / SHAKE_HIGHPASS_HZ).round() as usize).max(1);
    let low = running_mean(&rates.iter().map(|r| r.1).collect::<Vec<_>>(), lp);
    let hf2: Vec<f64> = rates.iter().zip(&low).map(|(r, l)| (r.1 - l).norm_squared()).collect();
    let window = ((SHAKE_WINDOW_S * rate_hz).round() as usize).max(1);
    running_mean_scalar(&hf2, window).into_iter().zip(&rates).map(|(v, r)| (r.0, v.sqrt())).collect()
}

/// The inclusive index ranges where `mask` is set
fn runs(mask: &[bool]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &m) in mask.iter().enumerate() {
        match (m, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => { out.push((s, i - 1)); start = None; }
            _ => {}
        }
    }
    if let Some(s) = start { out.push((s, mask.len() - 1)); }
    out
}

fn running_mean(x: &[Vector3<f64>], window: usize) -> Vec<Vector3<f64>> {
    let half = window / 2;
    let mut prefix = Vec::with_capacity(x.len() + 1);
    prefix.push(Vector3::zeros());
    for v in x { let last = prefix[prefix.len() - 1]; prefix.push(last + v); }
    (0..x.len()).map(|i| {
        let (a, b) = (i.saturating_sub(half), (i + half + 1).min(x.len()));
        (prefix[b] - prefix[a]) / (b - a) as f64
    }).collect()
}
fn running_mean_scalar(x: &[f64], window: usize) -> Vec<f64> {
    let half = window / 2;
    let mut prefix = Vec::with_capacity(x.len() + 1);
    prefix.push(0.0);
    for v in x { let last = prefix[prefix.len() - 1]; prefix.push(last + v); }
    (0..x.len()).map(|i| {
        let (a, b) = (i.saturating_sub(half), (i + half + 1).min(x.len()));
        (prefix[b] - prefix[a]) / (b - a) as f64
    }).collect()
}

/// Widens the marks by the margin, clamps them to the data, and merges the overlapping and close ones
fn merge(mut marks: Vec<(i64, i64, Reason)>, data: &TimeQuat, settings: &FindSettings) -> Vec<Stretch> {
    let (Some((&first, _)), Some((&last, _))) = (data.first_key_value(), data.last_key_value()) else { return Vec::new(); };
    let margin = (settings.margin_s * 1e6) as i64;
    let gap = (settings.merge_gap_s * 1e6) as i64;
    marks.sort_by_key(|m| m.0);
    let mut out: Vec<Stretch> = Vec::new();
    for (a, b, reason) in marks {
        let (a, b) = ((a - margin).max(first), (b + margin).min(last));
        if b <= a { continue; }
        match out.last_mut() {
            Some(s) if a <= s.end_us + gap => {
                s.end_us = s.end_us.max(b);
                if !s.reasons.contains(&reason) { s.reasons.push(reason); s.reasons.sort(); }
            }
            _ => out.push(Stretch { start_us: a, end_us: b, reasons: vec![reason] }),
        }
    }
    out
}

/// The orientation of `quats` at `t`: interpolated between the samples around it, held past the ends
pub fn orientation_at(quats: &TimeQuat, t: i64) -> Option<Quat64> {
    let before = quats.range(..=t).next_back();
    let after = quats.range(t..).next();
    match (before, after) {
        (Some((&ta, qa)), Some((&tb, qb))) if tb > ta => {
            let f = (t - ta) as f64 / (tb - ta) as f64;
            Some(qa.try_slerp(qb, f, 1e-9).unwrap_or(*qa))
        }
        (Some((_, q)), _) | (None, Some((_, q))) => Some(*q),
        (None, None) => None,
    }
}

/// Puts the orientations measured from the image (`video`, any sampling) in place of the motion data's within
/// `start_us..=end_us`. The image's motion is taken relative to its orientation at the start, from the motion data's
/// orientation there, and what it drifted from the motion data's orientation at the end is spread over the stretch, so
/// both ends meet the motion data. Returns false and leaves `quats` as they were when either has nothing there
pub fn splice(quats: &mut TimeQuat, start_us: i64, end_us: i64, video: &TimeQuat) -> bool {
    if end_us <= start_us { return false; }
    let (Some(qa), Some(qb)) = (orientation_at(quats, start_us), orientation_at(quats, end_us)) else { return false; };
    let (Some(va), Some(vb)) = (orientation_at(video, start_us), orientation_at(video, end_us)) else { return false; };
    let to_motion = qa * va.inverse();
    // Where the image's motion alone ends up, and the rotation left from there to the motion data's end, in the
    // camera's own frame
    let drift = (to_motion * vb).inverse() * qb;
    let span = (end_us - start_us) as f64;
    for (t, q) in quats.range_mut(start_us..=end_us) {
        let Some(v) = orientation_at(video, *t) else { continue; };
        let f = (*t - start_us) as f64 / span;
        let spread = UnitQuaternion::identity().try_slerp(&drift, f, 1e-9).unwrap_or(UnitQuaternion::identity());
        *q = to_motion * v * spread;
    }
    true
}

/// The motion measured from the image in each stretch, as `GyroSource::integrate` splices it in
#[derive(Clone, Debug, Default)]
pub struct SafeModeMotion {
    /// Off: the stretches and their motion are kept (the timeline still shows them), but the motion data is used as is
    pub enabled: bool,
    pub stretches: Vec<Stretch>,
    /// The orientations measured from the image for each stretch, on the motion data's timeline. `None` for a stretch
    /// the image couldn't be tracked in: the motion data stays there
    pub video: Vec<Option<TimeQuat>>,
}
impl SafeModeMotion {
    /// Splices every stretch with motion from the image into `quats`. Returns how many were
    pub fn apply(&self, quats: &mut TimeQuat) -> usize {
        if !self.enabled || quats.len() < 2 { return 0; }
        self.stretches.iter().zip(&self.video)
            .filter(|(s, v)| v.as_ref().is_some_and(|v| splice(quats, s.start_us, s.end_us, v)))
            .count()
    }
    pub fn hash_into(&self, hasher: &mut impl std::hash::Hasher) {
        hasher.write_u8(self.enabled as u8);
        for (s, v) in self.stretches.iter().zip(&self.video) {
            hasher.write_i64(s.start_us);
            hasher.write_i64(s.end_us);
            hasher.write_usize(v.as_ref().map_or(0, |v| v.len()));
        }
    }
}

/// The orientations measured in a safe-mode analysis of one stretch (`OpticalMotionAnalysis::for_safe_mode`), on the
/// motion data's timeline: the motion chained between the frames, refined by the correction fitted with `settings`,
/// moved from the video's time by the sync offset there
pub fn video_motion(m: &crate::synchronization::optical_motion::OpticalMeasurements, settings: &crate::gyro_source::OpticalCorrectionSettings, gyro: &crate::gyro_source::GyroSource) -> Result<TimeQuat, String> {
    let correction = crate::synchronization::optical_motion::solve(m, settings)?;
    if correction.video_base.len() < 2 { return Err("Not enough of the image could be tracked".into()); }
    let mut quats = correction.base_quats();
    correction.apply(&mut quats);
    Ok(quats.into_iter().map(|(t, q)| (t - (gyro.offset_at_video_timestamp(t as f64 / 1000.0) * 1000.0).round() as i64, q)).collect())
}

/// A one-line summary for the UI, in English (translated in QML): how many stretches, how long, and why
pub fn summary(stretches: &[Stretch]) -> serde_json::Value {
    let mut reasons: Vec<Reason> = stretches.iter().flat_map(|s| s.reasons.iter().copied()).collect();
    reasons.sort();
    reasons.dedup();
    serde_json::json!({
        "count": stretches.len(),
        "seconds": stretches.iter().map(Stretch::duration_s).sum::<f64>(),
        "reasons": reasons,
        "stretches": stretches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gyrogate::dsp;

    fn to_quat(q: &[f64; 4]) -> Quat64 { Quat64::from_quaternion(nalgebra::Quaternion::new(q[0], q[1], q[2], q[3])) }

    /// 1 kHz attitude, each value stored twice like the O4 does, with a smooth motion and optional false bursts on the
    /// Z axis at the given samples
    fn o4_track(seconds: f64, bursts_at: &[usize]) -> TimeQuat {
        let n = (seconds * 1000.0) as usize;
        let rates: Vec<[f64; 3]> = (0..n).map(|i| {
            let t = i as f64 / 1000.0;
            let burst: f64 = bursts_at.iter().map(|&b| if (b..b + 15).contains(&i) { 150.0 * (std::f64::consts::PI * (i - b) as f64 / 15.0).sin() } else { 0.0 }).sum();
            [25.0 * (4.4 * t).sin(), -18.0 * (2.5 * t).sin(), 12.0 * (5.6 * t).cos() + burst]
        }).collect();
        dsp::integrate([0.9, 0.1, -0.3, 0.2], &rates, 1000.0).iter().enumerate()
            .flat_map(|(i, q)| [(i as i64 * 1000, to_quat(q)), (i as i64 * 1000 + 500, to_quat(q))])
            .collect()
    }

    #[test]
    fn clean_motion_has_no_stretch() {
        let data = o4_track(6.0, &[]);
        assert!(find(&data, Some(&Default::default()), &Default::default()).is_empty());
    }

    #[test]
    fn isolated_false_rotations_stay_with_the_repaired_motion_data() {
        // The Gyrogate repair takes care of these: running the image analysis for each would only cost time
        let data = o4_track(8.0, &[2000, 2600, 5600]);
        assert!(find(&data, Some(&Default::default()), &Default::default()).is_empty());
    }

    #[test]
    fn without_the_gyrogate_detector_a_smooth_clip_is_left_alone() {
        let data = o4_track(8.0, &[2000]);
        // The generic shake detector doesn't take one 15 ms burst for shake
        assert!(find(&data, None, &Default::default()).is_empty());
    }

    #[test]
    fn dense_shake_is_marked_on_any_camera() {
        // 200 Hz camera, smooth pan with 40 Hz shake of 1.5° amplitude between 3 and 5 s
        let data: TimeQuat = (0..2000).map(|i| {
            let t = i as f64 / 200.0;
            let shake = if (3.0..5.0).contains(&t) { 1.5f64.to_radians() * (2.0 * std::f64::consts::PI * 40.0 * t).sin() } else { 0.0 };
            ((t * 1e6) as i64, Quat64::from_euler_angles(shake, 0.3 * t, 0.0))
        }).collect();
        let found = find(&data, None, &Default::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].reasons, vec![Reason::Shake]);
        assert!(found[0].start_us <= 3_000_000 && found[0].start_us >= 2_300_000, "{found:?}");
        assert!(found[0].end_us >= 5_000_000 && found[0].end_us <= 5_700_000, "{found:?}");
        assert_eq!(summary(&found)["count"], 1);
    }

    #[test]
    fn a_short_jolt_is_not_a_stretch() {
        // The same shake for a quarter second: the motion data can be trusted with it
        let data: TimeQuat = (0..2000).map(|i| {
            let t = i as f64 / 200.0;
            let shake = if (3.0..3.25).contains(&t) { 1.5f64.to_radians() * (2.0 * std::f64::consts::PI * 40.0 * t).sin() } else { 0.0 };
            ((t * 1e6) as i64, Quat64::from_euler_angles(shake, 0.3 * t, 0.0))
        }).collect();
        assert!(find(&data, None, &Default::default()).is_empty());
    }

    fn path(seconds: f64, hz: f64, f: impl Fn(f64) -> Quat64) -> TimeQuat {
        (0..=(seconds * hz) as usize).map(|i| { let t = i as f64 / hz; ((t * 1e6).round() as i64, f(t)) }).collect()
    }

    #[test]
    fn splice_follows_the_image_and_meets_the_motion_data_at_both_ends() {
        // The true camera pans; the motion data has a wrong wobble in 2-4 s; the image measured the true motion but
        // from another reference and with a small drift
        let truth = |t: f64| Quat64::from_euler_angles(0.1 * t, 0.4 * t, 0.05 * t);
        let mut quats = path(6.0, 1000.0, |t| {
            let wobble = if (2.0..4.0).contains(&t) { 0.05 * (2.0 * std::f64::consts::PI * 7.0 * t).sin() } else { 0.0 };
            truth(t) * Quat64::from_euler_angles(wobble, 0.0, 0.0)
        });
        let reference = Quat64::from_euler_angles(0.7, -0.2, 1.1);
        let video = path(6.0, 30.0, |t| reference * truth(t) * Quat64::from_euler_angles(0.0, 0.002 * t, 0.0));

        let before = quats.clone();
        assert!(splice(&mut quats, 2_000_000, 4_000_000, &video));
        // Untouched outside
        for (t, q) in quats.range(..2_000_000).chain(quats.range(4_000_001..)) { assert_eq!(q, &before[t]); }
        // Continuous at the ends
        assert!(quats[&2_000_000].angle_to(&before[&2_000_000]) < 1e-9);
        assert!(quats[&4_000_000].angle_to(&before[&4_000_000]) < 1e-9);
        // The wobble is gone: within the stretch it stays close to the true motion
        let worst = quats.range(2_000_000..=4_000_000).map(|(t, q)| q.angle_to(&truth(*t as f64 / 1e6)).to_degrees()).fold(0.0, f64::max);
        let wobble = before.range(2_000_000..=4_000_000).map(|(t, q)| q.angle_to(&truth(*t as f64 / 1e6)).to_degrees()).fold(0.0, f64::max);
        assert!(worst < 0.2 && wobble > 2.5, "after {worst}°, before {wobble}°");
    }

    #[test]
    fn safe_mode_motion_splices_only_when_enabled_and_measured() {
        let truth = |t: f64| Quat64::from_euler_angles(0.0, 0.4 * t, 0.0);
        let wobble = |t: f64| if (1.0..3.0).contains(&t) { 0.05 * (2.0 * std::f64::consts::PI * 7.0 * t).sin() } else { 0.0 };
        let quats = path(4.0, 1000.0, |t| truth(t) * Quat64::from_euler_angles(wobble(t), 0.0, 0.0));
        let video = path(4.0, 30.0, truth);
        let stretches = vec![
            Stretch { start_us: 1_000_000, end_us: 3_000_000, reasons: vec![Reason::Shake] },
            Stretch { start_us: 3_500_000, end_us: 3_900_000, reasons: vec![Reason::Tremor] },
        ];
        let mut motion = SafeModeMotion { enabled: false, stretches, video: vec![Some(video), None] };
        let mut q = quats.clone();
        assert_eq!(motion.apply(&mut q), 0);
        assert!(q == quats);
        motion.enabled = true;
        assert_eq!(motion.apply(&mut q), 1);
        let worst = q.range(1_000_000..=3_000_000).map(|(t, q)| q.angle_to(&truth(*t as f64 / 1e6)).to_degrees()).fold(0.0, f64::max);
        assert!(worst < 0.1, "{worst}");
    }

    #[test]
    fn splice_needs_both_sources() {
        let mut quats = path(2.0, 100.0, |t| Quat64::from_euler_angles(0.0, t, 0.0));
        assert!(!splice(&mut quats, 500_000, 1_000_000, &TimeQuat::new()));
        let same = quats.clone();
        assert!(!splice(&mut quats, 1_000_000, 500_000, &same));
    }

    /// Prints the shake level and the stretches found on a real clip, to tune the thresholds. Run with
    /// `SPDS_CLIP=<file> cargo test -p gyroflow-core --release safe_mode_on_a_real_clip -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn safe_mode_on_a_real_clip() {
        use std::sync::{ Arc, atomic::AtomicBool };
        use crate::gyro_source::{ GyroSource, FileLoadOptions };
        let Ok(clip) = std::env::var("SPDS_CLIP") else { return; };
        let path = std::path::Path::new(&clip);
        let mut file = std::fs::File::open(path).unwrap();
        let size = file.metadata().unwrap().len();
        let md = GyroSource::parse_telemetry_file(&mut file, size, path, &FileLoadOptions::default(), (3840, 2160), 60.0, |_| (), Arc::new(AtomicBool::new(false))).unwrap();
        let unit = gyrogate::unit::classify(&md.additional_data);
        println!("unit: {unit:?}");
        let data = md.quaternions;
        println!("{} attitude samples, {:.1} s", data.len(), data.keys().last().map_or(0.0, |&t| (t - data.keys().next().unwrap()) as f64 / 1e6));

        let t0 = *data.keys().next().unwrap();
        let profile = shake_profile(&data);
        let mut per_s: std::collections::BTreeMap<i64, (f64, f64, usize)> = Default::default();
        for &(t, v) in &profile {
            let e = per_s.entry((t - t0) / 1_000_000).or_insert((0.0, 0.0, 0));
            e.0 = e.0.max(v); e.1 += v; e.2 += 1;
        }
        println!("second: max / mean shake RMS (deg/s)");
        for (s, (mx, sum, n)) in &per_s { println!("{s:4} {mx:7.1} {:7.1}", sum / *n as f64); }

        let gs = gyrogate::Settings::default();
        let rel = |t: i64| (t - t0) as f64 / 1e6;
        if let Some(sum) = gyrogate::memory::analyze(&data, &gs) { println!("gyrogate: {sum:?}"); }
        for threshold in [30.0, 40.0, 50.0, 60.0] {
            let found = find(&data, Some(&gs), &FindSettings { shake_rms_dps: threshold, ..Default::default() });
            let list: Vec<String> = found.iter().map(|s| format!("{:.1}-{:.1} {:?}", rel(s.start_us), rel(s.end_us), s.reasons)).collect();
            println!("threshold {threshold}: {} stretches, {:.1} s: {}", found.len(), found.iter().map(Stretch::duration_s).sum::<f64>(), list.join(", "));
        }
    }
}
