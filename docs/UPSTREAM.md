# Paridad completa y mantenimiento de nightly

Decisión del propietario, 2026-11-02: ampliar el objetivo a paridad completa con T3 Code,
con dos ramas permanentes y mayor estabilidad que el canal nightly.

## Ramas

- `dev`: integración de ports, correcciones y adaptación de contratos. Las ramas de trabajo
  parten de `dev` y regresan mediante PR con validación.
- `main`: promoción explícita de un SHA de `dev` validado; sin sincronización automática
  con upstream. Conservar la versión estable anterior y sus instrucciones de rollback.
- No actualizar servicios, perfiles diarios o clientes instalados desde el monitor.

La separación comienza en el SHA existente de main; no certifica retrospectivamente su estabilidad.
Las protecciones de GitHub deben exigir PR y el workflow `Validation` en ambas ramas;
main además requiere revisión y resolución de conversaciones. La configuración remota debe
verificarse antes de afirmar que estas reglas están impuestas.

## Registro verificable

`compat-status.json` y `COMPATIBILITY.md` siguen siendo el registro de ports por RPC y
comando, con evidencia y límites. `done` significa lo que documenta cada fila; una prueba
negativa o un decoder que acepta la forma no basta para certificar comportamiento.
`skip` pasa a backlog de paridad completa. No cambiarlo masivamente a done ni perder la
prioridad de los consumidores actuales. HTTP, CLI, suscripciones, proveedores, recuperación,
perfiles, dispositivos, browser y UI necesitan evidencia adicional en CONFORMANCE/E2E.

La referencia contractual sigue fijada en 3726 hasta validar una actualización explícita.
El registro de nightly detectada es distinto de la referencia validada: `nightly/report.json`
con SHA completo, publicación, RPC añadidos/retirados, archivos y commits por intervalo.
El escáner de nombres RPC es orientativo; no detecta cambios semánticos ni todos los schemas.

## Automatización

`tools/nightly-watch` consulta releases publicadas de GitHub y compara las cinco nightly
más recientes contra el commit fijado y entre sí. Lee código mediante un repositorio bare
en `/var/tmp/o3-build`; nunca ejecuta ni instala código upstream, ni cambia el checkout diario.
Un error de red o de resolución de referencia falla la ejecución; no cuenta como “sin cambios”.

GitHub Actions ejecuta el monitor cada seis horas y manualmente, conserva la evidencia
como artefacto y actualiza un único issue de intake. Los cron sólo se ejecutan cuando el
workflow existe en la rama predeterminada de GitHub: mientras main no reciba esta entrega,
el proceso está preparado pero no activo. No llamar a esta automatización un port automático.

Por cada nightly nueva:

1. Revisar los cambios de contratos, servidor y clientes; abrir tareas concretas para cada
   cambio relevante, incluida cualquier superficie nueva fuera del inventario 3726.
2. Añadir o reproducir primero una regresión; implementar en una rama contra dev.
3. Ejecutar pruebas en ambos modos bridge, Clippy, fmt, tabla generada y diferencial contra
   la referencia anterior y la candidata, con perfiles aislados y sin credenciales personales.
4. Verificar flujos positivos reales desktop/Android y recuperación/reconexión. Medir RAM,
   CPU y latencia con el perfil real y uno grande; incluir hijos de proveedor/browser por separado.
5. Promover filas sólo con evidencia; registrar pendientes y fijar los SHAs de server/lib/clientes.
6. Promover dev a main mediante PR revisado tras superar las comprobaciones. Un fallo
   funcional, de seguridad o de presupuesto bloquea promoción. Objetivo servidor <50 MB,
   techo <100 MB; documentar por separado procesos externos y escenarios que exceden el objetivo.

## Orden de trabajo

Primero cerrar las correcciones y auditorías pendientes de clientes (serialización de shell,
validación de origen e identidad de tareas), sin dar por terminadas ramas históricas.
Después cerrar P0/P1 parciales; completar browser/proveedores y flujos nativos pendientes;
por último ampliar las superficies antes omitidas y las incorporadas por nightly reciente.
La paridad total continúa pendiente hasta demostrar estos comportamientos, no sólo inventariarlos.

## Intake verificado — 2026-11-02

La API oficial identifica `v0.1.7-nightly.20261102.4093`, SHA
`84537c4dd976a89b6ec094f43ae3ef7bee12cb1a`, como la más reciente de esta consulta.
Contra 3726 se detectan 17 nombres RPC añadidos y 14 retirados; conservar los antiguos
para los clientes fijados hasta diseñar una negociación de capacidades compatible.
Hay 1988 archivos afectados en las superficies vigiladas; ese conteo no equivale a 1988 tareas.

Prioridad de revisión del delta más reciente: secretos de source-control mediante proveedor,
ruteo de agentes en background a delegación, límites de lecturas de recuperación y aislamiento
de sesiones Claude. También revisar tags de hilos, cambios de modelo/esfuerzo y estado visible
tras actualizar proveedores (4066). Ningún cambio queda marcado portado por esta inspección.
