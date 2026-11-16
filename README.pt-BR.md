<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="resources/brand/logo-horizontal-escuro.svg">
    <img alt="SP Drone Stab" src="resources/brand/logo-horizontal-claro.svg" width="560">
  </picture>
</p>

<p align="center">
  <b>Estabilização de vídeo FPV pelo giroscópio, gratuita e de código aberto.</b><br>
  <a href="README.md">Read in English</a>
</p>

---

O SP Drone Stab estabiliza vídeos FPV usando os dados de movimento que a câmera grava. Ele foi feito para quem voa com
a DJI O4 e, principalmente, para as unidades da O4 cujo giroscópio grava movimentos falsos, o que faz as correções
comuns deixarem o vídeo pulando ou com jello.

> **Situação: primeira versão de teste.** A versão de macOS é a testada. A de Windows também é publicada, mas ainda não
> foi testada.

## O que ele faz

- **Detecta sozinho o problema do giroscópio da O4.** Lê a data de fabricação da unidade pelo número de série gravado no
  vídeo e avisa se ela provavelmente tem o giroscópio ICM-40609-D (DJI O4 Pro fabricadas a partir de fevereiro de 2026 e
  todas as O4 Lite), que grava rotações falsas com a vibração dos motores.
- **Conserta os dados de movimento antes de estabilizar.** Remove as rotações falsas e o tremor em um eixo só que esse
  giroscópio grava. Leva uma fração de segundo e não precisa decodificar o vídeo. Também pode salvar uma cópia corrigida
  do arquivo (`<nome>_gyrofix.MP4`).
- **Modo seguro para os piores trechos.** Onde a câmera chacoalha forte por vários segundos, nem os dados consertados
  são confiáveis. O SP Drone Stab encontra esses trechos sozinho, estabiliza eles pela imagem em vez do giroscópio e usa
  o giroscópio no resto do vídeo. A linha do tempo marca esses trechos em âmbar, e o app explica em palavras simples o
  que fez. É a mesma estabilização só pela imagem do "Ignore motion data from the file" do Gyroflow, mas apenas onde
  precisa: o resto do vídeo mantém a precisão do giroscópio e não precisa da análise de imagem, que é mais lenta.
- **Cor com LUT, direto na exportação.** Gravou em D-Log M ou outro perfil plano? Carregue o LUT `.cube` dele no
  painel **Cor** e o arquivo exportado já sai com as cores normais, sem precisar passar de novo pelo editor. Tem controle
  de intensidade, uma pasta com os seus LUTs a um clique e 13 ajustes (exposição, contraste, realces, sombras, balanço
  de branco, saturação, nitidez, vinheta e outros). Trazido do FPV Flow.
- **Tudo o que o Gyroflow 1.7.0 faz**: perfis de lente, correção de rolling shutter, correção e sincronização óptica,
  algoritmos de suavização, renderização na placa de vídeo, keyframes e fila de renderização.

## Download

- **macOS (Apple Silicon e Intel):** [SP-Drone-Stab-mac.dmg](https://github.com/fpvnicolas-byte/sp-drone-stab/releases/latest/download/SP-Drone-Stab-mac.dmg).
  Abra e arraste o **SP Drone Stab** para Aplicativos. Ele fica ao lado do Gyroflow, não substitui. O app não é
  assinado pela Apple, então na primeira vez o macOS bloqueia a abertura: vá em **Ajustes do Sistema > Privacidade e
  Segurança** e clique em **Abrir Mesmo Assim**.
- **Windows (ainda não testado):** [SP-Drone-Stab-windows64.zip](https://github.com/fpvnicolas-byte/sp-drone-stab/releases/latest/download/SP-Drone-Stab-windows64.zip).
  Extraia em qualquer pasta e abra o `Gyroflow.exe`.

Todas as versões ficam na [página de Releases](https://github.com/fpvnicolas-byte/sp-drone-stab/releases). O Gyroflow
oficial fica em [gyroflow.xyz](https://gyroflow.xyz); este não é ele.

## Problemas e sugestões

Abra uma issue [aqui](https://github.com/fpvnicolas-byte/sp-drone-stab/issues). Por favor, não leve problemas do SP
Drone Stab aos projetos Gyroflow ou FPV Flow: eles não mantêm este app.

## De onde ele vem

O SP Drone Stab é construído sobre o trabalho destes projetos de código aberto, e não existiria sem eles:

- **[Gyroflow](https://github.com/gyroflow/gyroflow)**, de [Adrian Eddy](https://github.com/AdrianEddy) e
  colaboradores, GPL-3.0 ou posterior. O motor de estabilização e quase todo o app vêm do Gyroflow 1.7.0. Veja o
  [README do Gyroflow](https://github.com/gyroflow/gyroflow#readme) para a lista completa de recursos, câmeras
  suportadas, autores e agradecimentos.
- **[FPV Flow](https://github.com/rodrigosclosa/fpvflow)**, de Rodrigo Sclosa, GPL-3.0 ou posterior. A detecção e o
  conserto do Gyrogate (`src/core/gyrogate`) e o painel de cor com LUTs `.cube` e ajustes (`src/core/color`,
  `src/ui/menu/ColorLut.qml`) foram trazidos do FPV Flow.
- **[o4fix](https://github.com/vadimbryl/o4fix)**, de vadimbryl, MIT. O conserto do FPV Flow é uma adaptação do o4fix.

## Como compilar

O SP Drone Stab compila do mesmo jeito que o Gyroflow: veja as
[instruções de desenvolvimento do Gyroflow](https://github.com/gyroflow/gyroflow#development) e use este repositório.

## Licença

O SP Drone Stab é distribuído sob a licença GPLv3 com exceção para lojas de aplicativos, como o Gyroflow. Veja
[LICENSE](LICENSE). O texto em inglês do [README](README.md#license) é o que vale para as permissões adicionais
(distribuição do `gyroflow_core` em lojas de aplicativos e uso do mdk-sdk, que é de código fechado e tem licença
própria).

Os avisos de copyright dos autores do Gyroflow e do FPV Flow continuam nos arquivos de código que eles escreveram.
