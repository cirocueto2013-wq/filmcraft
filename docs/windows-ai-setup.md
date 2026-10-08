# FilmCraft AI para Windows x64 — instalación y configuración con Codex

Este paquete contiene **nuestra versión modificada**, con asistente, Autopilot, voces Kokoro,
ComfyUI y las herramientas AI/MCP. También conserva las correcciones de Save As y exportación
atómica. Es una compilación independiente, basada en FilmCraft; no es un instalador oficial de
ArtCraft. `BUILD_INFO.json` identifica el commit y los hashes de los archivos.

## Instalar el editor

Windows 10/11 de 64 bits, Intel/AMD. Abrí el `.msi`, o descomprimí el ZIP portable **completo**.
El MSI instala en `%LOCALAPPDATA%\Programs\FilmCraftAI` y agrega **FilmCraft AI (Codex)** al
menú Inicio. No reemplaza FilmCraft oficial. El ZIP funciona desde su carpeta, sin instalación.
La compilación independiente no tiene certificado de firma; Windows puede mostrar el aviso
correspondiente. No desactives Defender ni cambies globalmente las políticas de PowerShell.

Abrí **FilmCraft-Codex.cmd**. Inicia el editor con el puente local `127.0.0.1:9876`; los datos de
esta edición quedan en `%LOCALAPPDATA%\FilmCraftAI\AppData`. Los proyectos y sus medios se
guardan donde elijas. Conservá los medios generados junto al proyecto al moverlo a otra PC.

## Conectar este Codex, usando la suscripción de ChatGPT

Sí: el chat que usaste para desarrollar FilmCraft es Codex. En tu computadora, Codex se conecta
al editor a través de MCP. **No se copia la cuenta ni el token de este chat al editor.** Usá el
login existente de Codex con tu cuenta de ChatGPT y los límites de tu suscripción.

Desde PowerShell, dentro de la carpeta instalada/extraída:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\Configure-Codex.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\Verify-Installation.ps1
codex mcp get filmcraft-ai
```

`Configure-Codex.ps1` requiere el CLI `codex` disponible en PATH. Si sólo tenés la aplicación
de escritorio y no encuentra el CLI, instalá Node.js LTS y luego `npm install -g @openai/codex`,
o pasá `-CodexCommand` con la ruta a tu `codex.exe`. Comprobá `codex login status`; si hace falta,
`codex login` permite iniciar sesión con ChatGPT. El script no modifica el login, el modelo,
los permisos ni otros servidores; respalda `config.toml` y registra sólo `filmcraft-ai`.
Respeta un `CODEX_HOME` que ya tengas configurado. No hace falta una API key para este flujo.

Reiniciá Codex para cargar el servidor y mantené FilmCraft AI abierto. Pedile al chat que use
`project_inspect`, `sequence_inspect`, `assistant_preview`/`assistant_apply` y las herramientas
de generación para editar el proyecto visible. Las operaciones normales permiten undo/redo.
Para editar directamente desde Codex, el propio Codex planifica y llama las herramientas:
no es necesario que `ai.plan` haga una segunda llamada a OpenAI.

Estos lanzadores configuran **Codex nativo en Windows**. Si tu otro chat trabaja dentro de WSL,
que compruebe qué configuración/runtime está usando antes de registrar el servidor: una ruta
Windows y el localhost del host no equivalen automáticamente a los del entorno Linux/WSL.

La sección **AI connections** del asistente integrado sigue usando una clave de API para
planificación cloud dentro del editor. La suscripción de ChatGPT no paga esa API. Con Codex por
MCP podés dejar esa clave vacía. No guardes claves, cookies ni tokens en este README o en el config.

## Instalar voces y ComfyUI

Los servicios y modelos se instalan **aparte**, bajo tu control. No están ocultos en el MSI ni
usan la facturación de OpenAI. Las descargas pueden ocupar varios GB. La versión predeterminada
usa CPU; no exige NVIDIA.

- Kokoro: Docker Desktop funcionando en modo contenedores Linux. El script usa una imagen CPU
  oficial fijada por digest, publica sólo `127.0.0.1:8880` y no reemplaza contenedores ajenos.
- ComfyUI: Python 3.12 y Git en PATH. El script crea un entorno Python aislado y descarga un
  commit fijado de ComfyUI en `%LOCALAPPDATA%\FilmCraftAI\Services`. No cambia tu Python global
  ni un ComfyUI existente. Los nodos personalizados quedan desactivados en el lanzador por defecto.

Para el otro chat, si faltan herramientas, los paquetes de Windows son `Git.Git`,
`Python.Python.3.12`, `OpenJS.NodeJS.LTS` y `Docker.DockerDesktop` en winget. Instalá sólo los que
falten. Docker puede requerir habilitar virtualización/WSL2 y reiniciar: comprobá ese estado,
sin asumir que ya está listo. No instales Rust ni Visual Studio para usar este paquete.

```powershell
# Revisar las acciones antes de descargar:
powershell -NoProfile -ExecutionPolicy Bypass -File .\Setup-Services.ps1 -Kokoro -ComfyUI -WhatIf
# Instalar los servicios solicitados:
powershell -NoProfile -ExecutionPolicy Bypass -File .\Setup-Services.ps1 -Kokoro -ComfyUI
# Iniciar y verificar:
powershell -NoProfile -ExecutionPolicy Bypass -File .\Start-Services.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\Verify-Installation.ps1 -Services
```

En FilmCraft: **Window → Text → Assistant**, cargar las voces reales, elegir `ef_dora` para
español o `af_heart` para inglés, generar e importar/agregar al timeline. Para comprobar ComfyUI,
pegar `comfy-workflow.json` en el campo de workflow API; genera una imagen de prueba con nodos
reales del servidor sin necesitar un checkpoint. Para generación neuronal de imágenes/video,
el otro chat debe instalar modelos compatibles con el workflow que elijas y revisar su licencia;
este paquete no promete un modelo ni un workflow de difusión que no incluya.

Si usás servicios ya instalados, evitá los scripts de instalación y configurá sus URLs en
**AI connections** / `ai_configure`. Para detener sólo los servicios creados por estos scripts:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\Stop-Services.ps1
```

Desinstalar el MSI quita los binarios/scripts y el acceso directo. Conserva tus proyectos,
medios, configuración de Codex y servicios descargados. Para quitar la conexión, usá
`codex mcp remove filmcraft-ai`. No borres carpetas de medios para desinstalar la aplicación.

## Instrucción lista para el otro chat de Codex

> Instalá y configurá el paquete FilmCraft AI x64 que descargué. Leé este README y BUILD_INFO.json.
> Verificá los hashes y ejecutá el CLI para confirmar que incluye ai.autopilot. Usá el MSI o el
> portable, conservando la versión oficial y mis proyectos. Configurá sólo el servidor MCP
> filmcraft-ai con Configure-Codex.ps1 y mi sesión existente de ChatGPT; no solicites ni copies
> API keys ni tokens para ese flujo. Instalá los prerrequisitos que falten y ejecutá Setup-Services
> para Kokoro y ComfyUI, después Start-Services. Abrí FilmCraft-Codex.cmd, verificá MCP/servicios
> y probá una voz en español y otra en inglés, el workflow ComfyUI incluido, importar/editar,
> undo/redo, guardar/reabrir y exportar. No desactives protecciones globales de Windows.

## Reconstruir este paquete

Fuentes: [rama Windows AI](https://github.com/cirocueto2013-wq/filmcraft/tree/build/windows-ai-bundle).
En un host Windows con Rust MSVC, SDK y WiX v5: `packaging/windows/package.ps1 -Arch x64 -AiBundle`.
En Linux con MinGW/wixl: compilar ambos binarios para `x86_64-pc-windows-gnu` con CRT estático y
`FILMCRAFT_FORK_BUILD=1`, y ejecutar `packaging/windows/create_ai_bundle.py --help`.
El paquete no incluye FFmpeg, un runtime Python, Docker, credenciales ni pesos fine-tuned.
Detalles de acciones y límites: `AI-MCP.md` (en el paquete), o [ai-and-mcp.md](ai-and-mcp.md).
