# Especificación de AI Factory

AI Factory es una app desktop con motor Rust, host Tauri y UI React en inglés con tema oscuro. Ejecuta workflows visuales sobre eventos GitHub mediante Codex CLI u OpenCode CLI. Requiere Git y los agentes elegidos; no requiere Python ni `gh`.

El diseño completo está en [desktop](docs/superpowers/specs/2026-11-11-desktop-design.md). Las pruebas y límites de validación están en [verificación desktop](docs/desktop-verification.md).

## Triggers GitHub

- `new_issue`: issues abiertas cuyo título empieza por `[AI]`, con owner y autor permitidos. El autor selecciona el perfil de token GitHub de ejecución.
- `pr_approved`: únicamente PRs publicadas y verificadas por un workflow de issue. Requiere una review `APPROVED` del head SHA actual o un comentario nuevo exactamente `OK`, tras quitar espacios e ignorar mayúsculas. El autor del comentario debe coincidir con el autor de la PR y el autor de la issue origen. Se ejecuta con la identidad del autor de la issue.
- `pr_review`: PRs abiertas con una review solicitada a un reviewer permitido. Esta lista es independiente de los autores de issues y no exige procedencia de `new_issue`. El reviewer selecciona su perfil GitHub. Tras observar retirada de una solicitud, una solicitud nueva puede generar otro evento. Una lista vacía de reviewers omite la consulta.

La deduplicación es duradera en SQLite. Aprobación y comentario sobre el mismo head comparten un evento. Los IDs de comentarios consumidos no se reutilizan tras reinicios ni cambios de head. Un head distinto requiere una aprobación de ese commit o un comentario nuevo. Un error al consultar comentarios no consume sus IDs; una señal válida de aprobación o comentario puede crear el evento aunque falle la otra consulta.

## Workflows y ejecución

Cada trigger tiene un DAG editable y un interruptor independiente. Los nodos definen agente, modelo, esfuerzo, acceso `read` o `write`, prompt, JSON Schema de resultado, dependencias y posición visual. Se rechazan ciclos, dependencias inexistentes y schemas inválidos al guardar.

Cada run conserva una copia inmutable de su workflow. Los cambios de configuración se aplican a nuevos runs. El límite global predeterminado es cuatro runs entre los tres triggers; los demás quedan en cola. Cada run dispone de workspace Git y memoria mutable aislados. Los workflows PR se fijan al head SHA del evento. Los nodos esperan el éxito de sus dependencias y los escritores se serializan dentro de cada run.

Los resultados contienen `status`, `summary`, `artifacts` y `data`. Los estados válidos son `succeeded`, `blocked` y `failed`; `data` debe cumplir el schema del nodo. Un resultado exitoso no puede declarar `data.approved=false`. Los artifacts incluyen ruta, tipo y SHA256; se verifican existencia, contenido y ubicación autorizada.

Los agentes solicitan acciones GitHub mediante `data`; el motor las valida y ejecuta con el token del actor:

- `publish: {title, body, base}`: solo en `new_issue`, con resultado exitoso. Publica la rama del evento y crea o adopta su PR. El body debe cerrar la issue origen con `Closes #N`.
- `merge: {method: "squash"}`: solo en `pr_approved`, con resultado exitoso y head SHA coincidente.
- `review: {event, body}`: solo en `pr_review`. Acepta `APPROVE`, `REQUEST_CHANGES` o `COMMENT`; un resultado bloqueado no puede aprobar.
- `comment: "texto"`: permite informar resultados exitosos o bloqueados.

Las acciones se validan antes de mutar Git o GitHub. Los reintentos reconcilian PRs y merges existentes y conservan recibos para comentarios y reviews. La cancelación impide nuevas acciones y detiene los procesos de la sesión, incluidos sus descendientes. Un cierre inesperado marca runs inciertos como `interrupted`; el reintento explícito crea un intento nuevo con el workflow actual.

## Aplicación y datos

Overview, Runs, Workflows y Settings muestran datos reales, estados vacíos, errores y operaciones en curso. Los eventos nativos actualizan listas, detalles y logs; la suscripción precede a la lectura inicial y una pérdida de eventos fuerza resincronización.

Settings administra perfiles de token, owners, autores, reviewers, repositorios locales, comandos, intervalos, timeouts, concurrencia y arranque al iniciar sesión. Los tokens guardados no se devuelven a la UI y se redactan en logs. Git recibe credenciales por proceso, sin cambiar la configuración global del usuario.

SQLite, configuración, workspaces, memoria y sesiones residen en el directorio de datos de la aplicación del sistema operativo. Cerrar la ventana mantiene el motor en la bandeja; Quit detiene el motor y sus procesos.

El importador implementado en Rust admite YAML y SQLite anteriores, abre la DB origen en modo lectura y conserva los datos disponibles de historial, sesiones, resultados, logs, memoria, procedencia de PRs y deduplicación. Es idempotente y convierte trabajo pendiente o activo en interrumpido. No incluye ni ejecuta el daemon anterior.

Las instrucciones de instalación, pruebas y empaquetado están en [README](README.md). Los workflows CI generan NSIS Windows x64, app/DMG macOS Apple Silicon y deb/AppImage Ubuntu x64.
