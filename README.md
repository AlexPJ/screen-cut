<div align="center">

# ✂️ ScreenCut

### Capturas de pantalla ultraligeras para Windows — con OCR, captura con scroll y anotaciones

[![Release](https://img.shields.io/github/v/release/AlexPJ/screen-cut?style=for-the-badge&color=d97757)](https://github.com/AlexPJ/screen-cut/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/AlexPJ/screen-cut/total?style=for-the-badge&color=d97757)](https://github.com/AlexPJ/screen-cut/releases)
[![License](https://img.shields.io/github/license/AlexPJ/screen-cut?style=for-the-badge&color=d97757)](LICENSE)
[![Windows](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D6?style=for-the-badge&logo=windows&logoColor=white)](#)
[![Rust + Tauri](https://img.shields.io/badge/Rust%20%2B%20Tauri-2-000?style=for-the-badge&logo=tauri&logoColor=white)](#)

**[⬇️ Descargar la última versión](https://github.com/AlexPJ/screen-cut/releases/latest)**

<img src="assets/hero.png" alt="ScreenCut en acción" width="820" />

</div>

---

ScreenCut hace lo mismo que «Recortes» de Windows, pero **más rápido, más ligero y con superpoderes**: reconocimiento de texto (OCR) preciso, capturas con scroll que unen páginas enteras, y un editor de anotaciones completo. El ejecutable ronda los **2–3 MB** y consume **~25 MB de RAM**. Sin Electron, sin navegador empaquetado, sin dependencias pesadas.

## ✨ Características

- 🖼️ **Captura de región, ventana o pantalla completa** — overlay sobre el escritorio congelado; arrastra para seleccionar. Multi-monitor y con soporte de escalado (DPI).
- 📜 **Captura con scroll (Snagit-style)** — vertical y horizontal. Recorre páginas o conversaciones largas y las cose en una sola imagen, detectando el desplazamiento píxel a píxel. Tú decides cuándo parar.
- 🔤 **OCR preciso con Tesseract** — extrae el texto de cualquier captura, incluso terminales de fondo oscuro (preprocesado con inversión y contraste automáticos). Copia el texto con un clic.
- 🎨 **Editor de anotaciones** — flechas, líneas, recuadros, elipses, texto, resaltado y dibujo a mano. Color y grosor a elegir, relleno (ninguno / color del trazo / otro color), mover y redimensionar, borrador, **deshacer/rehacer** y **recorte (crop)**.
- ⏱️ **Temporizador** configurable (3 s por defecto) con cuenta atrás.
- ⌨️ **Atajo global** — `Ctrl+Shift+X`, o convierte **Impr Pant (Print Screen)** en tu herramienta de captura por defecto.
- 🔔 **Vive en la bandeja del sistema** — siempre lista, opción de iniciar con Windows.
- 🌗 **Tema claro/oscuro** con la preferencia recordada.
- 🔄 **Actualizaciones automáticas** firmadas, integradas en la app.
- 💾 **Copia al portapapeles** o **guarda como PNG** el resultado (con anotaciones incluidas).

## 📸 Capturas

**OCR — reconoce hasta terminales de fondo oscuro, con rutas y símbolos intactos:**

<img src="assets/ocr.png" alt="OCR con Tesseract" width="720" />

**Ajustes y actualizaciones (tema oscuro):**

<img src="assets/settings.png" alt="Ajustes y Acerca de" width="620" />

## ⬇️ Descarga e instalación

1. Ve a la **[página de releases](https://github.com/AlexPJ/screen-cut/releases/latest)**.
2. Descarga `ScreenCut_x.y.z_x64-setup.exe`.
3. Ejecútalo. Windows SmartScreen puede advertir por ser un editor desconocido: *Más información → Ejecutar de todas formas*.

> Requisitos: Windows 10/11 (x64). WebView2 viene incluido en Windows 11 y en la mayoría de Windows 10 actualizados.

Una vez instalada, la app se actualizará sola: **Ajustes → Acerca de → Buscar actualizaciones**.

## 🚀 Uso rápido

| Acción | Cómo |
| --- | --- |
| Capturar una región | Botón **Región** o `Ctrl+Shift+X` (o Impr Pant si lo activas) |
| Pantalla completa | Botón **Pantalla** |
| Captura con scroll | **Scroll vertical/horizontal** → selecciona la zona → **Terminar** cuando quieras |
| Extraer texto (OCR) | Botón **OCR** |
| Anotar | Barra de herramientas superior (flecha, recuadro, texto…) |
| Recortar | Herramienta **crop** ⌏ |
| Guardar / copiar | **Guardar** (PNG) o **Copiar** (portapapeles) |

## 🛠️ Compilar desde el código

Requisitos: [Rust](https://rustup.rs) (rustup), VS Build Tools con C++, y [Tesseract](https://github.com/UB-Mannheim/tesseract/wiki) para el OCR.

```powershell
git clone https://github.com/AlexPJ/screen-cut.git
cd screen-cut/src-tauri
cargo build --release              # exe en target/release/screen-cut.exe
# Instalador NSIS:
cargo install tauri-cli --locked
cargo tauri build
```

El perfil release está optimizado para tamaño y RAM (`opt-level="z"`, LTO, `strip`, `panic=abort`).

### Arquitectura (clean, modular)

```
src-tauri/src/
  core/     Tipos de dominio (RawImage, OcrResult…) — sin dependencias de plataforma
  infra/    Adaptadores Windows: capture (GDI), ocr (Tesseract + preprocesado),
            scroll (SendInput + stitching), clipboard (Win32), png_io
  app/      Estado y comandos Tauri (orquestación)
ui/         Frontend estático (sin Node ni bundler): HTML/CSS/JS por capas
```

## 🔤 OCR

Usa **Tesseract** (motor LSTM) llamado como proceso externo, con preprocesado (grises, inversión automática en fondos oscuros, contraste, ampliación ×2). Si Tesseract no está, cae al motor nativo de Windows. Busca `tesseract.exe` junto al ejecutable, en el `PATH`, o en las rutas de instalación estándar. Para distribuir de forma autónoma, incluye la carpeta de Tesseract (con `tessdata`) junto al `.exe`.

## 🔄 Publicar una nueva versión (mantenedores)

<details>
<summary>Flujo de release + firma del updater</summary>

1. Sube el número de versión en `src-tauri/tauri.conf.json` y `Cargo.toml`.
2. Compila **firmando** (clave privada `src-tauri/screencut.key`, nunca subir al repo):
   ```powershell
   $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content src-tauri\screencut.key -Raw
   $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
   cargo tauri build
   ```
   Genera en `src-tauri/target/release/bundle/nsis/`: `*-setup.exe` y su firma `*-setup.exe.sig`.
3. Crea un **Release de GitHub** (etiqueta `vX.Y.Z`) y sube el `*-setup.exe` y un `latest.json`:
   ```json
   {
     "version": "0.2.0",
     "notes": "Novedades…",
     "pub_date": "2026-01-01T00:00:00Z",
     "platforms": {
       "windows-x86_64": {
         "signature": "<contenido completo de *-setup.exe.sig>",
         "url": "https://github.com/AlexPJ/screen-cut/releases/download/v0.2.0/ScreenCut_0.2.0_x64-setup.exe"
       }
     }
   }
   ```

La app instalada compara su versión con la de `latest.json` (`.../releases/latest/download/latest.json`) y ofrece actualizar.

</details>

## 📄 Licencia

[MIT](LICENSE) © Alejandro Padilla

<div align="center">
<sub>Hecho con Rust + Tauri. Ligero por diseño.</sub>
</div>
