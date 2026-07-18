# ScreenCut

Herramienta de capturas de pantalla para Windows, súper ligera (Rust + Tauri 2, frontend estático sin Node). Hace lo mismo que "Recortes" de Windows, con OCR nativo y captura con scroll estilo Snagit.

## Funciones

- **Captura de región** — overlay a pantalla completa sobre el escritorio congelado, arrastra para seleccionar. Atajo global: `Ctrl+Shift+X`.
- **Pantalla completa** — captura todo el escritorio virtual (multi-monitor).
- **Captura con scroll** (vertical y horizontal) — selecciona la zona, la app envía eventos de rueda y cose los fotogramas detectando el desplazamiento por coincidencia de píxeles.
- **OCR** — motor nativo de Windows (`Windows.Media.Ocr`): sin descargas, usa los idiomas instalados en el sistema.
- **Anotaciones** — flechas, líneas, recuadros, elipses, texto, resaltado (marcatextos) y dibujo a mano; color (paleta + personalizado), relleno (sin relleno / del color del trazo / de otro color) y grosor. Herramienta de selección para **mover y redimensionar** (handles en las esquinas), **borrador** para eliminar trazos al pasar, `Supr` borra la selección, recorte (crop) y deshacer/rehacer (`Ctrl+Z` / `Ctrl+Y`). Copiar/guardar/OCR operan sobre el resultado aplanado.
- **Temporizador** configurable (0 / 3 / 5 / 10 s; 3 s por defecto, preferencia recordada) con cuenta atrás antes de capturar.
- **Copiar imagen** al portapapeles (CF_DIB), **guardar PNG**, copiar texto reconocido.
- **Tema claro/oscuro** con preferencia recordada (localStorage). Barra de título propia que sigue el tema (la nativa de Windows no puede) e iconos SVG inline (sin fuentes ni frameworks: cero sobrecoste).

## Arquitectura (clean, modular)

```
src-tauri/src/
  core/     Tipos de dominio (RawImage, CaptureInfo, OcrResult) — sin dependencias de plataforma
  infra/    Adaptadores Windows: capture (GDI), ocr (WinRT), scroll (SendInput + stitching),
            clipboard (Win32), png_io
  app/      Estado y comandos Tauri (orquestación)
ui/         Frontend estático: index.html (ventana principal), overlay.html (selección de región)
```

## Compilar

Requisitos: Rust (rustup), VS Build Tools con C++, WebView2 (incluido en Windows 11).

```powershell
cd src-tauri
cargo build --release   # exe standalone en target/release/screen-cut.exe
```

Para generar el instalador NSIS: `cargo install tauri-cli --locked` y luego `cargo tauri build`.

El perfil release está optimizado para tamaño y RAM (`opt-level="z"`, LTO, strip).

## OCR con Tesseract

El OCR usa **Tesseract** (motor LSTM) llamado como proceso externo, con preprocesado de imagen (escala de grises, inversión automática en fondos oscuros, contraste y ampliación ×2). Si Tesseract no está disponible, cae al motor nativo de Windows.

La app busca `tesseract.exe` en este orden: junto al ejecutable (`tesseract\tesseract.exe`), el `PATH`, `C:\Program Files\Tesseract-OCR`, y `%LOCALAPPDATA%\Programs\Tesseract-OCR`. Para **distribuir** la app de forma autónoma, copia la carpeta de Tesseract (con `tessdata`) junto al `.exe` o inclúyela como recurso del bundle. Idiomas: usa `eng` (+`spa` si añades `spa.traineddata` a `tessdata`).

## Como app predeterminada (Impr Pant) y bandeja

- En **Ajustes** puedes activar **«Usar Impr Pant»**: registra Print Screen como atajo global para capturar una región y desactiva el mapeo de Windows a «Recortes» para que gane este atajo.
- **«Iniciar con Windows»** arranca la app en segundo plano al iniciar sesión.
- Al **cerrar** la ventana, la app se oculta en la **bandeja del sistema** y sigue respondiendo al atajo. Para salir del todo: clic derecho en el icono de bandeja → **Salir**.

## Publicar y actualizaciones automáticas

La app integra el **updater de Tauri**: comprueba un `latest.json` remoto, descarga el instalador firmado y lo aplica (con reinicio). En **Ajustes → Acerca de → Buscar actualizaciones**.

Configuración (ya hecha):
- Par de claves de firma en `src-tauri/screencut.key` (privada, **NO subir**, ya en `.gitignore`) y `screencut.key.pub` (pública, embebida en `tauri.conf.json`).
- `plugins.updater.endpoints` apunta a `https://github.com/OWNER/screen-cut/releases/latest/download/latest.json` — **reemplaza `OWNER`** por tu usuario/organización de GitHub.

Para publicar una versión nueva:
1. Sube el número en `src-tauri/tauri.conf.json` y `Cargo.toml` (p. ej. `0.2.0`).
2. Compila firmando:
   ```powershell
   $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content src-tauri\screencut.key -Raw
   $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
   cargo tauri build
   ```
   Genera en `src-tauri/target/release/bundle/nsis/`: el instalador `*-setup.exe` y su firma `*-setup.exe.sig`.
3. Crea un **Release de GitHub** con la etiqueta `v0.2.0` y sube el `*-setup.exe` y un `latest.json`:
   ```json
   {
     "version": "0.2.0",
     "notes": "Novedades…",
     "pub_date": "2026-01-01T00:00:00Z",
     "platforms": {
       "windows-x86_64": {
         "signature": "<contenido de *-setup.exe.sig>",
         "url": "https://github.com/OWNER/screen-cut/releases/download/v0.2.0/ScreenCut_0.2.0_x64-setup.exe"
       }
     }
   }
   ```
   El `signature` es el **texto completo** del archivo `.sig`. La `url` apunta al `*-setup.exe` de ese release.

La app instalada comparará su versión con la del `latest.json` y ofrecerá actualizar. (El endpoint `.../releases/latest/download/latest.json` siempre resuelve al último release publicado.)
