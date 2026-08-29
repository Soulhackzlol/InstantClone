<div align="center">

<sub><a href="README.md">English</a> &nbsp;·&nbsp; <b>Español</b></sub>

<br/>
<br/>

<img src="docs/preview.svg" alt="InstantClone, proxy RTMP libre y de código abierto para delay de stream en OBS y multistream (simulcast)" width="100%"/>

<br/>

<a href="https://github.com/Soulhackzlol/InstantClone/releases/latest"><img alt="Descargar para Windows" src="https://img.shields.io/badge/Descargar%20para%20Windows-5ac8fa?style=for-the-badge&labelColor=11141a&logo=windows&logoColor=white"/></a>


<a href="#inicio-rápido"><img src="https://img.shields.io/badge/-Inicio%20r%C3%A1pido-1c2129?style=for-the-badge&labelColor=11141a"/></a>
<a href="#funciones"><img src="https://img.shields.io/badge/-Funciones-1c2129?style=for-the-badge&labelColor=11141a"/></a>
<a href="#cómo-funciona"><img src="https://img.shields.io/badge/-C%C3%B3mo%20funciona-1c2129?style=for-the-badge&labelColor=11141a"/></a>
<a href="#control-http"><img src="https://img.shields.io/badge/-API%20HTTP-1c2129?style=for-the-badge&labelColor=11141a"/></a>

<br/>
<br/>

<sub><a href="https://youtu.be/y3aj88gTAOs"><b>▶ Mira el tutorial de instalación</b></a> &nbsp;·&nbsp; Audio en español, subtítulos en inglés</sub>

<br/>
<br/>

<a href="https://github.com/Soulhackzlol/InstantClone/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/Soulhackzlol/InstantClone/ci.yml?branch=main&style=flat-square&label=ci&color=34c759&labelColor=11141a"/></a>
<a href="https://github.com/Soulhackzlol/InstantClone/releases"><img alt="release" src="https://img.shields.io/github/v/release/Soulhackzlol/InstantClone?include_prereleases&style=flat-square&color=5ac8fa&labelColor=11141a&display_name=tag&sort=semver"/></a>
<a href="LICENSE"><img alt="GPL-3.0" src="https://img.shields.io/badge/license-GPL--3.0-d4d8e1?style=flat-square&labelColor=11141a"/></a>
<img alt="Windows y Linux" src="https://img.shields.io/badge/plataforma-windows%20%7C%20linux-7a7d8a?style=flat-square&labelColor=11141a"/>
<a href="https://alternativeto.net/software/instantclone/about/"><img alt="En AlternativeTo" src="https://img.shields.io/badge/AlternativeTo-listado-5ac8fa?style=flat-square&labelColor=11141a&logo=alternativeto&logoColor=white"/></a>

</div>

<br/>

<div align="center">

### Un delay de stream sin cortes para OBS.

Una señal entra. Un delay con buffer que **armas**, **activas** y **cortas** al vuelo, repartido a todas las plataformas a la vez. Libre y de código abierto.

</div>

<br/>

<div align="center">
<img src="docs/pipeline.svg" alt="Una señal de OBS entra al buffer en anillo en disco de InstantClone, se retiene N segundos y se reparte a Twitch, YouTube, Kick y RTMP personalizado a la vez" width="100%"/>
</div>

<br/>

<div align="center"><img src="docs/divider.svg" alt="" width="100%"/></div>

## Inicio rápido

<sub>El primer arranque abre un asistente que te guía por todo esto. Los pasos de abajo son lo mismo a mano.</sub>

<table>
<tr>
<td valign="top" width="50%">

**1 · Ejecútalo**

```text
Descarga instantclone.exe → doble clic.
El panel abre en http://127.0.0.1:7799
```

Esa es toda la instalación. Un icono queda en la bandeja mientras corre; clic derecho para el panel, el dock, un **Corte** de un clic, o **Salir**. Cerrar la pestaña no mata el proxy, solo Salir lo hace.

<sub>Al primer arranque, Windows SmartScreen puede decir "editor desconocido" porque la build aún no está firmada. Pulsa **Más información → Ejecutar de todas formas**, o compárala con el `SHA256SUMS.txt` de la release.</sub>

</td>
<td valign="top" width="50%">

**2 · Apunta OBS aquí**

Pulsa **Registrar con OBS** en el panel, reinicia OBS una vez y en **Ajustes → Emisión** elige:

```text
Servicio:  InstantClone
Clave:     main          (vale cualquier texto)
```

El modo multipista "Auto" funciona de fábrica. Tus claves reales van en la pestaña **Destinos**, no en OBS.

<sub>¿Prefieres manual? Servicio <b>Personalizado</b>, Servidor <code>rtmp://localhost:1935/live</code>, Clave <code>main</code>.</sub>

</td>
</tr>
</table>

**3 · Arma, activa, corta**

<div align="center">
<img src="docs/states.svg" alt="Tres pasos para un delay de stream: Armar llena el buffer mientras sigues en directo, Activar pasa a diferido sin cortes, Cortar vuelve a directo al instante" width="100%"/>
</div>

<table>
<tr><td align="center" width="40"><b>1</b></td><td>Escribe un delay (p. ej. <kbd>15</kbd>s) y pulsa <b>Armar</b>. El buffer se prellena desde la señal en directo sin tocar lo que sale.</td></tr>
<tr><td align="center"><b>2</b></td><td>Cuando ponga <code>ARMED</code>, pulsa <b>Activar</b>. El cambio a diferido es instantáneo en pantalla, sin reconexión ni corte.</td></tr>
<tr><td align="center"><b>3</b></td><td><b>Corta</b> para volver a directo cuando quieras, o <b>&#9201; Cortar cuando esto salga</b> para autocortar justo cuando tu reacción llega a los espectadores. Sin contar el delay de cabeza.</td></tr>
</table>

> [!IMPORTANT]
> El Firewall de Windows preguntará al primer arranque porque el proxy escucha en <code>:1935</code> (RTMP) y <code>:7799</code> (web). Permítelo solo en **redes privadas**.

> [!WARNING]
> Windows 10/11 y Linux (x86-64) están soportados. En Linux funciona headless en un VPS o en un escritorio Ubuntu; el panel web del navegador es la superficie de control (no hay bandeja nativa). macOS todavía no está soportado.

<br/>

<div align="center"><img src="docs/divider.svg" alt="" width="100%"/></div>

## Por qué

Quería un buffer de delay para mi propio stream y me puse a buscar. La opción pulida que encontré fue [InstantDelay](https://instant-delay.com/), que es de pago. Prefería algo que pudiera reconstruir desde cero, entender de punta a punta y adaptar a mi setup, así que lo escribí yo.

Una vez existía, acabaron dentro las partes que de verdad quería: un armado/activado real de dos fases (para que el momento de salir con delay sea **sin cortes** en el reproductor del destino), varios destinos de salida a la vez (así hace también de herramienta de multistream / simulcast gratuita, una alternativa autoalojada a Restream), un dock de OBS y un overlay de estadísticas que puedes soltar como fuente de navegador.

<sub>InstantClone es un proyecto independiente, no afiliado ni respaldado por InstantDelay ni sus desarrolladores.</sub>

<br/>

<div align="center"><img src="docs/divider.svg" alt="" width="100%"/></div>

## Funciones

<table>
<tr>
<td valign="top" width="50%">

**🎯 Multistream a cada plataforma**
Haz simulcast de una señal de OBS a Twitch, YouTube, Kick y RTMP personalizado a la vez, una alternativa gratuita y autoalojada a Restream. Activa cada uno por separado y mira el bitrate por destino en vivo. Un **sink de prueba local** emite a un receptor diminuto en tu PC para ensayar armar/activar/cortar sin clave y sin que nada salga de tu máquina.

</td>
<td valign="top" width="50%">

**⏱ Corte programado seguro**
**Cortar cuando esto salga** marca el borde en directo y autocorta cuando ha llegado a los espectadores en todos los destinos. Ideal para reacciones de final de partida sin hacer cuentas de delay.

</td>
</tr>
<tr>
<td valign="top">

**📱 Vertical (9:16) gratis**
Activa **Formato Dual** de Twitch (Enhanced Broadcasting) y pon el formato de cualquier destino no-Twitch en **Vertical**. InstantClone reutiliza el lienzo 9:16 que OBS ya crea para Twitch y lo envía a YouTube Shorts, Kick móvil o TikTok, sin codificación extra.

</td>
<td valign="top">

**🎚 Audio de VOD + ruteo sin copyright**
Mantén la música en directo pero fuera de la grabación. Un clic añade una segunda pista de audio (con un script de OBS); luego, por destino, eliges la **pista de audio**: Twitch se queda **Ambas**, envías la **Pista 2** limpia a YouTube para esquivar copyright, o la **Pista 1** a Kick.

</td>
</tr>
<tr>
<td valign="top">

**📡 Passthrough de Enhanced Broadcasting**
Cuando OBS pasa a multipista "Auto", InstantClone hace de proxy de la config de Twitch, rutea al endpoint IVS de la sesión y reenvía cada SPS/PPS por pista fielmente para que se encienda la escalera de transcodificado. Los destinos no-Twitch reciben una sola pista limpia y aplanada.

</td>
<td valign="top">

**🔒 Salida RTMPS (Kick + cualquier `rtmps://`)**
El socket de salida sube a TLS para URLs `rtmps://`, reutilizando el schannel de Windows ya enlazado, sin una segunda pila TLS. Kick es una plataforma de pega-tu-URL-de-servidor en el asistente, con la ruta `/app` añadida automáticamente.

</td>
</tr>
<tr>
<td valign="top">

**🎛 Dock de OBS + overlays sin código**
Un dock de control 280×340 vive dentro de OBS para no hacer alt-tab a media partida. La pestaña **Overlay** es un Studio: elige un overlay de estadísticas ya hecho, copia su URL, suéltalo en OBS como fuente de navegador, o rediséñalo en vivo.

</td>
<td valign="top">

**⚡ Ajuste de delay en vivo**
Rearma o ajusta el delay arriba/abajo sin desarmar primero, expuesto como un control **↻ Ajustar a Ns** de un solo valor. Consciente de la capacidad: rechaza un delay que el buffer no aguanta y te dice exactamente cuántos MB necesita.

</td>
</tr>
<tr>
<td valign="top">

**⌨ Atajos de teclado globales**
Asigna delay on/off, armar, activar, cortar y **cortar cuando esto salga** a una combinación que funciona con un juego en pantalla completa por delante. Toda combinación necesita un modificador para que nada se dispare a media partida, una que otra app ya ocupa se marca en su fila en vez de fallar en silencio, y una acción rechazada te llega como globo en la bandeja.

</td>
<td valign="top">

**🎹 Pads y controladoras MIDI**
Mapea las mismas cinco acciones a un pad o un knob, aprendidos pulsando el control en vez de escribir un número de nota. Cada mapeo recuerda de qué dispositivo vino, así dos controladoras pueden mover acciones distintas aunque manden la misma nota, y puedes acotar a qué dispositivo escucha InstantClone.

</td>
</tr>
</table>

> [!TIP]
> **Registro de OBS de un clic.** El asistente puede añadir una entrada "InstantClone" al desplegable de Servicio de OBS por ti (escribe `services.json` con un `.bak` antes, se refresca al cambiar el puerto y avisa "cierra OBS primero" cuando el archivo está bloqueado).

<br/>

<div align="center"><img src="docs/divider.svg" alt="" width="100%"/></div>

## Cómo funciona

<table>
<tr>
<td valign="top" width="50%">

**Dos fases por diseño.** **Armas** un buffer (un tamaño objetivo en segundos). InstantClone lo prellena desde la señal de OBS sin tocar lo que sale. Cuando se llena pulsas **Activar**, y el cambio a diferido es instantáneo en pantalla: el reproductor solo salta del borde en directo a un punto N segundos atrás.

</td>
<td valign="top" width="50%">

**Cortar es el mismo truco al revés.** Pulsas **Cortar**, InstantClone se alinea con el fotograma clave más cercano al borde en directo, ajusta las marcas de tiempo para que sigan contando hacia delante sin saltos, y continúa. Sin reconexión, sin fotograma negro, sin corte visible.

</td>
</tr>
</table>

> [!NOTE]
> El buffer del delay vive en disco y se reinicia cada vez que cierras la app, así que nada se acumula entre sesiones. Pide más delay del que cabe y la app te dice exactamente cuánto necesita en vez de atascarse.

<br/>

<div align="center"><img src="docs/divider.svg" alt="" width="100%"/></div>

## Control HTTP

<table>
<tr>
<td valign="top" width="62%">

| | Endpoint | Cuerpo | Qué hace |
|:---|---|---|---|
| <kbd>POST</kbd> | `/arm` | `ms=15000` | Empieza a llenar un buffer de 15 s. Aún no sale. |
| <kbd>POST</kbd> | `/activate` | | Activa el delay armado. <code>409</code> si no está listo. |
| <kbd>POST</kbd> | `/disarm` | | Cancela el armado, descarta el buffer sin salir. |
| <kbd>POST</kbd> | `/stop` | | Vuelve a directo (igual que el botón **Cortar**). |
| <kbd>POST</kbd> | `/cut-after` | | Marca el borde en directo; autocorta cuando sale en todos. |
| <kbd>POST</kbd> | `/cut-after/cancel` | | Descarta un corte programado pendiente. |
| <kbd>POST</kbd> | `/delay` | `ms=NNN` | De un tiro: arma y autoactiva en cuanto esté listo. |
| <kbd>GET</kbd> | `/state` | | Instantánea JSON puntual. |
| <kbd>GET</kbd> | `/events` | | Flujo de estado JSON por server-sent events. Solo push. |

</td>
<td valign="top" width="38%">

<br/>

**Receta de Stream Deck**

La acción **Web Request** habla POST form-encoded por defecto.

```text
URL:     http://127.0.0.1:7799/arm
Método:  POST
Cuerpo:  ms=15000
```

Armado de un botón. Añade `/activate` y `/stop` a otros dos botones para control total desde tu deck.

</td>
</tr>
</table>

<details>
<summary>Respuesta de ejemplo de <code>/state</code></summary>

```json
{
  "phase": "active",
  "armed_delay_ms": 15000,
  "current_delay_ms": 15040,
  "buffer_fill_ms": 15000,
  "ingest_alive": true,
  "egress_alive": true,
  "destinations_alive": 2,
  "destinations_total": 3,
  "bitrate_kbps": 6020,
  "stats": { "tags_sent": 184302, "bytes_sent": 1338294104, "cuts": 1 }
}
```

</details>

<br/>

<div align="center"><img src="docs/divider.svg" alt="" width="100%"/></div>

## Por dentro

<details>
<summary><b>Interioridades de RTMP + Enhanced Broadcasting</b></summary>

<br/>

- **Handshake RTMP con paridad total con OBS.** `connect` lleva la misma bolsa de capacidades de códec que envía librtmp (`audioCodecs=3191`, `videoCodecs=252`, `videoFunction=1`), el `fourCcList` de Enhanced-RTMP (AVC / HEVC / AV1 / VP9 / Opus / AC-3 / FLAC), `Set Chunk Size` antes de connect, `FCUnpublish → deleteStream` al cerrar, y RTMP Acknowledgement (BYTES_READ_REPORT) al umbral ventana/10 declarado por el par, en entrada y salida.
- **Passthrough de Enhanced Broadcasting a Twitch.** Cuando OBS pasa a multipista "Auto" hacemos de proxy de `GetClientConfiguration` de Twitch, ruteamos la salida al endpoint IVS asignado a la sesión y reenviamos cada SPS/PPS por pista fielmente para que se encienda la escalera de transcodificado sin importar el nivel de cuenta. Los destinos no-Twitch reciben la pista primaria horizontal por defecto; las etiquetas de escalera con `TrackId != 0` se descartan para evitar la tormenta de varios frames por PTS que hace caer el decodificador de YouTube. Los cortes de EB caen en el IDR de la pista primaria (no en el keyframe del peldaño que gane el `partition_point`) para que el decodificador del destino siempre tenga su ancla.
- **Selección del lienzo vertical (9:16).** El lienzo vertical se identifica decodificando el SPS de cada pista por orientación (retrato, mayor área) en vez de confiar en el JSON privado de sesión de Twitch, y se autocorrige según se activa/desactiva Formato Dual.
- **Audio de VOD de Twitch, desbloqueado en el servicio InstantClone.** OBS ata su pista de VOD al servicio llamado literalmente "Twitch" (`ServiceSupportsVodTrack == {"Twitch"}`), así que está bloqueada en el servicio InstantClone. Un pequeño script de OBS incluido (`optional-vod-unlocker.lua`, descargado desde el panel) engancha el mismo segundo codificador de audio que usaría la propia pista de VOD de OBS, sin la restricción. Su lector de formato coincide byte a byte con el `flv_packet_audio_ex` de OBS (`AudioPacketType` en el byte 0, `TrackId` en el byte 6). OBS 32.2+ necesita el script; OBS anterior puede usar la casilla de VOD Track integrada (escribimos `EnableCustomServerVodTrack` en el `user.ini` de OBS 32, con `global.ini` como respaldo).
- **Ruteo de audio por destino.** Las pistas no seleccionadas se descartan y la elegida se aplana a una etiqueta de una sola pista estándar (AAC reescrito al `0xAF` legado), espejando el `flatten_multitrack_video` del lado de vídeo. Si la pista elegida no se está enviando, cae a la pista en directo en vez de quedarse en silencio.

</details>

<details>
<summary><b>Buffer, compilación y cobertura de tests</b></summary>

<br/>

<table>
<tr><td><b>RSS inactivo</b></td><td align="right"><code>~9 MB</code></td><td width="24"></td><td><b>Hilos</b></td><td align="right"><code>1 tokio + 1 bandeja</code></td><td width="24"></td><td><b>Deps runtime</b></td><td align="right"><code>tokio, bytes, ureq</code></td><td width="24"></td><td><b>Tests</b></td><td align="right"><code>282 / 282</code></td></tr>
</table>

**Buffer.** En disco por defecto (`./instantclone.buf`, 500 MB ≈ 11 min a 6 Mbps, ≈ 6 min 50 s a 10 Mbps), fuera de la RAM porque puede llegar a cientos de MB. Lo único en RAM es el índice de IDR, ~1 MB para 10 minutos a 60 fps. El archivo se reinicia en cada apagado limpio, así que nada se acumula entre sesiones, y la interfaz se niega a armar un delay mayor del que cabe, con un motivo explícito "necesita ≥ N MB".

**Compilar.** Rust 1.74+ estable. Sin npm, sin submódulos, sin SDKs de plataforma.

```powershell
git clone <repo>
cd instantclone
cargo build --release
.\target\release\instantclone.exe
```

El HTML del panel se minifica + gzipea en tiempo de compilación con `build.rs` (`flate2`, solo build) y se incrusta en el binario; en ejecución se sirve con `Content-Encoding: gzip`. El script opcional de VOD para OBS también va incrustado y se entrega al navegador como descarga "Guardar como", así que siempre coincide con el binario en ejecución y no necesita red.

**E/S de disco síncrona en la ruta caliente de escritura al anillo, por elección.** La escritura con buffer aterriza en la caché de páginas del SO en microsegundos y el kernel vacía en segundo plano, así que la caché de páginas ya es el buffer asíncrono; el índice y los bytes avanzan bajo un solo lock para que un lector nunca vea una etiqueta cuyos bytes aún no están en disco.

**Tests.** `cargo test --release` cubre la máquina de estados (`arm → preparing → ready → active → cut`), detección de IDR de AVC + Enhanced-RTMP, AMF0 (incluido Strict Array + guardia de recursión), round-trip de settings, expulsión del buffer en anillo con protección de lecturas en vuelo, parseo HTTP, política CSRF, pre-flight de puerto, negociación de contenido, caché de cabeceras de secuencia por pista de Enhanced Broadcasting + selección de etiquetas por TrackId, audio multipista + ruteo por destino, parseo de orientación SPS para la selección vertical, el parcheador de `services.json`, el parser del check de actualizaciones, el SHA-256 hecho a mano (vectores NIST), el lector/escritor de chunk-stream RTMP, la máquina del corte programado, las tablas de atajos y de mapeos MIDI (incluido el dispositivo que distingue dos controladoras), y la descarga de autoactualización + verificación de checksum + intercambio del exe. **378 tests, todos en verde.**

</details>

<br/>

<div align="center"><img src="docs/divider.svg" alt="" width="100%"/></div>

## Estado

**Listo para uso diario en Windows.** Lo uso en mis propios streams, y un grupo creciente de streamers lo corre a diario también. CI ejecuta fmt + clippy (`-D warnings`) + 378 tests en cada push, y un commit etiquetado compila y publica una release con un `SHA256SUMS.txt` al lado (todavía sin certificado de firma de código, así que el SO puede avisar al primer arranque).

**Lo áspero, con honestidad**

- **Sin bandeja nativa en Linux.** En Windows hay un icono en la bandeja del sistema; en Linux la superficie de control es el panel web (Salir y Reiniciar están en su pestaña Sistema). macOS aún no está soportado.
- **La escalera de transcodificado no está garantizada sin EB.** Solo los Twitch Partner tienen slot de transcodificado siempre; el resto se queda en Source-Only, donde algunos decodificadores por hardware fallan por encima de ~8 Mbps (es el comportamiento de asignación de Twitch, no del proxy). Para una escalera garantizada usa Enhanced Broadcasting; si no, mantén el bitrate cerca de ~6000 Kbps.
- **Servidor HTTP hecho a mano.** Binario más pequeño que con `hyper`, pero soy dueño de toda la superficie HTTP. A revisar si crece.

> [!WARNING]
> Esto es un proyecto hobby que uso yo mismo, no un producto de proveedor. Si emites esports de pago, valídalo contra tu propio pipeline antes de confiar en él una noche de torneo.

<br/>

<div align="center"><img src="docs/divider.svg" alt="" width="100%"/></div>

## Preguntas frecuentes

<details>
<summary><b>¿Emitir a varias plataformas baja mi calidad?</b></summary>

<br/>

No. InstantClone reenvía la señal ya codificada de OBS a cada destino sin recodificar, así que todas las plataformas reciben la misma calidad que produjo OBS. Lo que sí consume es **ancho de banda de subida**: cada destino recibe el bitrate completo, así que tres destinos necesitan unas tres veces la subida. El panel muestra un aviso de "cuello de botella de subida" y sugiere mantener el bitrate de OBS por debajo del ~80% de tu subida.

</details>

<details>
<summary><b>¿El delay se aplica a todas las plataformas a la vez?</b></summary>

<br/>

Sí. El buffer está antes del reparto, así que cada destino reproduce desde la misma posición retardada. Armar, activar y cortar les afectan a todos juntos.

</details>

<details>
<summary><b>¿Cuánto delay puedo poner?</b></summary>

<br/>

El que aguante el buffer. El archivo de 500 MB por defecto son unos 11 minutos a 6 Mbps (menos a bitrates más altos). Puedes hacerlo más grande; el panel rechaza un delay que el buffer no aguanta y te dice cuántos MB necesita, así que nunca se atasca en silencio.

</details>

<details>
<summary><b>¿Recodifica o toca mi vídeo?</b></summary>

<br/>

No. El vídeo pasa tal cual, bit a bit. Lo único que se reescribe es el contenedor de audio cuando ruteas una pista concreta a un destino (el AAC se reescribe a la etiqueta legada que acepta cualquier ingest); las muestras de audio no se tocan.

</details>

<details>
<summary><b>¿Dónde van mis claves de stream reales?</b></summary>

<br/>

En la pestaña **Destinos** de InstantClone, nunca en OBS. OBS solo apunta a InstantClone con un servicio y una clave desechable; InstantClone guarda la clave real de cada plataforma y reparte tu señal a todas. Así tus claves están en un solo sitio y activas o desactivas destinos sin tocar OBS.

**Tus claves nunca salen de tu PC.** InstantClone no tiene servidores propios ni telemetría: las claves se guardan localmente en tu máquina y solo se envían a los servidores de ingest de las plataformas a las que elijas emitir. Ejecutar la app no nos envía nada; nunca vemos tus claves, tu stream ni ninguna otra cosa. Es de código abierto, así que puedes verificarlo tú mismo.

</details>

<details>
<summary><b>¿Puedo controlar el delay con un atajo de teclado o una controladora MIDI?</b></summary>

<br/>

Sí, ambas cosas. Cinco acciones - delay on/off, armar, activar, cortar a directo y **cortar cuando esto salga** - se asignan a un atajo de teclado global, a un pad o knob MIDI, o a los dos a la vez, en **Ajustes**. Los atajos funcionan con un juego en pantalla completa por delante, así no haces alt-tab a media partida, y toda combinación necesita un modificador (Ctrl, Alt, Shift o Win) para que una tecla suelta no dispare una acción de delay. Los mapeos MIDI se aprenden pulsando el control en vez de escribir un número de nota, y cada uno recuerda de qué dispositivo vino, así dos controladoras pueden mover acciones distintas. Por ahora solo en Windows.

</details>

<details>
<summary><b>¿Funciona en macOS o Linux?</b></summary>

<br/>

Windows 10/11 y **Linux (x86-64)** lo ejecutan. En Linux funciona headless en un VPS o en un escritorio Ubuntu, gobernado desde el panel web (no hay bandeja nativa, así que Salir y Reiniciar viven en la pestaña Sistema del panel). Para una instalación expuesta a la red, activa la contraseña opcional del panel y acompáñala de TLS mediante un proxy inverso. macOS aún no está soportado.

</details>

<br/>

<div align="center"><img src="docs/divider.svg" alt="" width="100%"/></div>

## Contacto

<p>
<a href="https://twitch.tv/s1moscs"><img alt="Twitch @s1moscs" src="https://img.shields.io/badge/Twitch-%40s1moscs-9146FF?style=flat-square&logo=twitch&logoColor=white&labelColor=11141a"/></a>
&nbsp;<a href="https://x.com/s1moscs"><img alt="X @s1moscs" src="https://img.shields.io/badge/X-%40s1moscs-000000?style=flat-square&logo=x&logoColor=white&labelColor=11141a"/></a>
&nbsp;<a href="https://youtube.com/@s1moscs"><img alt="YouTube @s1moscs" src="https://img.shields.io/badge/YouTube-%40s1moscs-FF0000?style=flat-square&logo=youtube&logoColor=white&labelColor=11141a"/></a>
&nbsp;<a href="https://discord.com/users/s1moscs"><img alt="Discord @s1moscs" src="https://img.shields.io/badge/Discord-%40s1moscs-5865F2?style=flat-square&logo=discord&logoColor=white&labelColor=11141a"/></a>
</p>

Pásate mientras lo construyo en directo, o charlemos del proyecto. Reportes de bugs e ideas → [Issues](https://github.com/Soulhackzlol/InstantClone/issues) y [Discussions](https://github.com/Soulhackzlol/InstantClone/discussions).

<br/>

<div align="center"><img src="docs/divider.svg" alt="" width="100%"/></div>

## Licencia

[GPL-3.0](LICENSE). Úsalo, modifícalo, córrelo en el stream que quieras. Si distribuyes una versión modificada (incluido un fork "Pro", un instalador con extras, o un front-end de pago), tu código fuente tiene que salir bajo la misma licencia, públicamente. Construí esto como alternativa libre porque quería una para mí; GPL es lo que mantiene libres los forks también.

Hecho por [s1moscs](https://s1moscs.dev).
