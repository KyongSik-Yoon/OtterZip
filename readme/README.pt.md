<div align="center">

<img src="images/otter.png" width="96" alt="OtterZip">

# OtterZip

**A ferramenta de arquivos discreta para o Windows e o Linux.**

Clique com o botão direito para compactar ou extrair. Sem anúncios, sem contas, sem rastreamento.

[**Site**](https://lumibearstudio.github.io/otterzip-web/) · [**Microsoft Store**](https://apps.microsoft.com/detail/9NWQNGGSWJCL) · [**Download**](https://github.com/LumiBearStudio/OtterZip/releases/latest/download/OtterZip_x64_installer.zip)

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [中文](README.zh.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md) · Português · [Русский](README.ru.md) · [Italiano](README.it.md)

</div>

<br>

<img src="images/window.png" alt="OtterZip main window">

## Zipe. Pronto.

Botão direito em um arquivo — está zipado. Botão direito em um arquivo compactado — está extraído. Você quase não abre o aplicativo.

## Por que o OtterZip

- **Direto do Explorador de Arquivos** — compacte e extraia direto do menu do botão direito. Sem precisar abrir uma janela antes.
- **Uma coisa, bem feita** — sem modos, sem excessos, sem curva de aprendizado.
- **Sem anúncios. Sem contas. Sem rastreamento.** Sem programas embutidos, sem insistência, nada para se cadastrar. Relatórios de falha só são enviados se você ativar.
- **Um núcleo nativo, discretamente rápido** — o motor é escrito em Rust. Ele faz o trabalho e não deixa você esperando.
- **Parece o Windows, porque é** — feito com C# e WinUI 3. Uma interface nativa de verdade, não uma página web dentro de uma janela. No Linux, uma janela Avalonia nativa sobre o mesmo motor.
- **Ajuste do seu jeito** — claro, escuro ou seguir o sistema. Dez idiomas incluídos.

## Instalação

### Microsoft Store — recomendado

Instalação com um clique, e as atualizações chegam automaticamente.

[Baixar na Microsoft Store](https://apps.microsoft.com/detail/9NWQNGGSWJCL)

### Download direto — gratuito

1. Baixe o [**OtterZip_x64_installer.zip**](https://github.com/LumiBearStudio/OtterZip/releases/latest/download/OtterZip_x64_installer.zip)
2. Extraia o zip.
3. Clique com o botão direito em `Install.ps1` → **Executar com o PowerShell** e aceite o aviso.

O pacote é assinado pela LumiBear Studio, e não pela Store, então a primeira instalação registra nosso certificado de publicador uma única vez — é para isso que serve o aviso. Guia passo a passo: [**Como instalar**](https://lumibearstudio.github.io/otterzip-web/install.html)

Os dois canais são exatamente o mesmo aplicativo.

### Linux

Baixe o tarball `linux-x64`, extraia e execute `./install.sh` — a instalação vai para `~/.local` e não precisa de root. Adicione o OtterZip ao menu de contexto do seu gerenciador de arquivos em **Configurações → Integração**.

Todos os detalhes, incluindo a linha de comando e as diferenças em relação ao Windows: [**OtterZip no Linux**](LINUX.md).

## Formatos

**Criar** — ZIP · 7z · TAR · TAR.GZ

**Extrair** — ZIP · 7z · RAR · TAR · TAR.GZ · GZ · BZ2 · XZ · ZST · LZ4 · ISO · CAB · JAR · APK · IPA e mais

Criptografia AES-256 e arquivos divididos (multivolume) quando você precisar.

## Padrões sensatos, desde o início

<img src="images/settings.png" width="620" alt="OtterZip settings">

Tema, idioma, regras de substituição e mais — ajustados como você quiser, e fora do caminho até você precisar.

## Requisitos

**Windows** — Windows 10 (versão 2004) ou posterior · x64

**Linux** — x64 ou arm64, X11 ou Wayland. O tarball de lançamento é autocontido, então não é preciso instalar nenhum runtime antes.

## Licença

- `crates/**` — **MIT OR Apache-2.0** (licença dupla do Rust)
- `app/**` — **GPL-3.0-or-later** (+ unRAR exception)

Consulte [LICENSING.md](../LICENSING.md) para detalhes.

## Avisos de terceiros

Consulte [THIRD-PARTY-NOTICES.md](../THIRD-PARTY-NOTICES.md). Em especial, o **unrar** (© Alexander Roshal) é usado apenas para extração de RAR — o OtterZip nunca cria arquivos RAR.

## Privacidade

Sem anúncios, sem contas, sem rastreamento. O envio de relatórios de falha é opcional e vem desativado por padrão. Consulte [PRIVACY.md](../PRIVACY.md).

---

<div align="center">

© 2026 LumiBear Studio

</div>
