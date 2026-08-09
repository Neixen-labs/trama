# TRAMA — Guía de arranque para IA (Claude Code)

> **Cómo usar este documento**: crea un repo vacío, copia este archivo como `KICKOFF.md` en la raíz,
> abre Claude Code y dile: *"Lee KICKOFF.md y ejecuta la Fase 1. Pide confirmación antes de cada
> decisión marcada como [DECISIÓN]"*. Las tareas marcadas **[HUMANO]** las haces tú, no la IA.

---

## 0. Contexto del proyecto (leer antes de tocar nada)

**Qué es TRAMA**: motor abierto de mapas de red. Tres pilares, todos obligatorios en el diseño:

1. **Formato binario abierto** pre-teselado con grafo de red e IDs estables. Un solo archivo,
   servible por HTTP range requests (patrón PMTiles), 100% funcional offline. Exportable a
   GeoJSON/GeoPackage/MVT — el anti-lock-in es requisito de producto, no un nice-to-have.
2. **Render GPU con eje temporal**: WebGPU con fallback WebGL2. El estado en tiempo real vive
   como textura GPU indexada por ID de entidad, con ring buffer temporal para scrub tipo vídeo
   e interpolación en shader. Fly-through 3D por la red (cámara sobre spline del grafo).
3. **Solvers como plugins** con contrato abierto: runtime local (WASM/WASI, sandboxed) y/o
   servidor. Primero EPANET (hidráulica); extensible a SWMM, pandapower, VRP. El núcleo es
   agnóstico de dominio: no sabe qué es una tubería.

**Qué NO es**: no es un GIS generalista (QGIS ya existe), no compite con MapLibre en basemaps
(lo usa como capa), no mete lógica de dominio en el núcleo.

**Posicionamiento**: "análisis de red para gente sin GIS". Open-core estilo Grafana/Supabase.
Licencia **BSL 1.1** en el core: uso interno permitido; servicio alojado o gestionado a terceros requiere licencia comercial. Primer vertical comercial: utilities de agua medianas.

**Restricciones técnicas del propietario** (respetar siempre):
- Backend Python: FastAPI + uvicorn + uv, SQLAlchemy 2.0 async, Pydantic v2.
- Frontend: Next.js App Router + Tailwind + shadcn/ui, TanStack Query.
- CI: Azure DevOps para lo privado; GitHub Actions para el open source.
- El compilador de datos se escribe en Python; el runtime de render en TypeScript.

**Naming**: org GitHub `Neixen-labs`, scope npm `@trama` (libres a fecha ago-2026).
PyPI `trama` está cogido → usar `trama-engine`. Dominio elegido: `trama.build` [HUMANO: registrar y añadir a Cloudflare].

---

## Fase 1 — Infraestructura y landing (1 día)

### [HUMANO] Antes de lanzar la IA
- [x] Crear org GitHub `Neixen-labs` y repo `trama` (público).
- [ ] Registrar `trama.build` y añadirlo a Cloudflare.
- [ ] Crear cuenta Formspree (o Buttondown) → obtener endpoint del formulario.
- [ ] Crear cuenta npm y reservar el scope `@trama` (publicar placeholder `@trama/core` v0.0.1).
- [ ] Cuenta Cloudflare Pages conectada al repo.

### [IA] Tareas
1. **Estructura de monorepo**:
   ```
   trama/
   ├── KICKOFF.md              # este documento
   ├── LICENSE                 # BSL 1.1
   ├── README.md               # pitch corto + estado del proyecto + link a lista de espera
   ├── docs/
   │   ├── POSITIONING.md      # (pegar el one-pager existente)
   │   ├── SPEC.md             # espec v0 del formato (Fase 2)
   │   └── SOLVER_CONTRACT.md  # contrato de solvers (Fase 2)
   ├── site/                   # landing (HTML existente `landing-trama.html`)
   ├── compiler/               # Python: datos → formato TRAMA (Fase 3)
   ├── engine/                 # TypeScript: runtime de render (Fase 4)
   └── solvers/
       └── epanet/             # primer plugin (Fase 5)
   ```
2. **Landing**: mover `landing-trama.html` a `site/index.html`, insertar el endpoint de
   Formspree en la constante `FORM_ENDPOINT`, verificar responsive y `prefers-reduced-motion`.
3. **CI mínimo**: GitHub Action que despliega `site/` a Cloudflare Pages en cada push a `main`.
4. **README** con: qué es (3 frases), los tres pilares, estado ("pre-alpha, espec en curso"),
   link a lista de espera, badge de licencia. Tono directo, cero marketing hueco.

**Criterio de hecho**: landing en producción bajo el dominio, formulario capturando emails,
repo público con README y licencia.

---

## Fase 2 — Espec v0 del formato + contrato de solvers (1-2 semanas)

### [IA] docs/SPEC.md — Formato TRAMA v0
Redactar especificación formal con estas secciones. [DECISIÓN] en cada punto abierto:

1. **Contenedor**: archivo único, little-endian. Header con magic bytes, versión, y directorio
   de secciones con offsets — diseñado para HTTP range requests (leer header + directorio +
   sección necesaria, nunca el archivo entero).
2. **Secciones v0** (solo estas — labels, raster y simbología compleja quedan explícitamente
   fuera hasta v1):
   - `GEOMETRY`: buffers pre-teselados (posiciones, índices) listos para `bufferData` directo,
     organizados en teselas jerárquicas (esquema de zoom tipo z/x/y). [DECISIÓN] cuantización
     de coordenadas (16-bit relativo a tesela recomendado).
   - `GRAPH`: nodos y aristas con IDs estables (u64), adyacencia CSR, referencia de cada arista
     a su geometría y orden de vértices (imprescindible para fly-through y solvers).
   - `PROPS`: propiedades tipadas por entidad, clave-valor con schema (tipos: f64, i64, string,
     bool, enum). Diccionario de claves global para compresión.
   - `STATE_CHANNELS`: declaración de canales de estado (nombre, tipo, unidad, rango) que los
     solvers escriben en runtime. El archivo NO contiene estado, solo el contrato.
3. **Compresión**: [DECISIÓN] zstd por sección (recomendado; brotli como alternativa).
4. **Exportadores**: especificar mapeo de ida y vuelta a GeoJSON y GeoPackage (obligatorio),
   MVT (solo ida). Pérdida aceptable documentada.
5. **Versionado**: semver de la espec; los archivos declaran versión mínima de lector.

### [IA] docs/SOLVER_CONTRACT.md — Contrato de solvers v0
1. **Manifiesto** `solver.toml`: id, versión, licencia, `runtimes = ["wasm", "server"]`,
   `[inputs]` (tipos de nodo/arista y props requeridas del grafo), `[outputs]` (canales de
   estado que escribe, con unidades), `[params]` (JSON Schema de configuración).
2. **API WASM**: interfaz WASI. Funciones exportadas: `init(graph_ptr, params_ptr)`,
   `step(t) -> state_delta_ptr`, `run(t0, t1) -> state_series_ptr`. Memoria compartida para
   el grafo (zero-copy). Sandbox: sin red, sin filesystem salvo el pasado explícitamente.
3. **API servidor**: mismo contrato sobre HTTP — `POST /solve` con referencia al archivo +
   params, respuesta streaming (SSE) de deltas de estado. Idéntico formato de delta que WASM:
   la app cliente no distingue de dónde viene el resultado.
4. **Delta de estado**: formato binario `(entity_id: u64, channel: u16, t: f32, value: f32)`
   empaquetado — es lo que alimenta la textura GPU del engine.
5. **Versionado del contrato**: los solvers declaran versión de contrato soportada.

**Criterio de hecho**: ambos documentos revisables por un tercero sin contexto previo, con
ejemplos hexdump/TOML completos. Publicar como PR para permitir comentarios de la comunidad.

---

## Fase 3 — Compilador Python (2-3 semanas)

### [IA] compiler/ — paquete `trama-engine` (PyPI)
- Stack: Python 3.12, uv, sin dependencias pesadas (shapely, pyproj, mapbox-earcut, zstandard;
  numpy para buffers). CLI con `typer`: `trama compile red.geojson -o red.trama`.
- Entradas v0: GeoJSON (líneas y polígonos), **EPANET .inp** (extrae grafo + props hidráulicas
  directamente — validación de ida y vuelta obligatoria: `.inp → .trama → .inp` sin pérdida
  funcional), CSV de puntos.
- Salida: archivo conforme a SPEC.md. Test golden: mismos datos → bytes idénticos (determinismo).
- Exportadores: `trama export red.trama --to geojson|gpkg`.
- CI: pytest + ruff + mypy en GitHub Actions. Cobertura del parser .inp con redes de ejemplo
  de EPANET (Net1, Net3).

**Criterio de hecho**: compilar una red municipal real (~50k tramos) en <30 s, archivo
resultante <20% del GeoJSON equivalente, ida y vuelta a .inp verificada.

---

## Fase 4 — Engine mínimo (3-4 semanas)

### [IA] engine/ — paquete `@trama/core` (npm)
- TypeScript, WebGPU con fallback WebGL2 (detección en runtime). Sin framework: librería pura
  con adaptadores (`@trama/react` después).
- v0 pinta SOLO: líneas del grafo coloreadas/dimensionadas por textura de estado + scrub
  temporal del ring buffer + cámara 2.5D con fly-through sobre spline de aristas. Nada de
  labels, nada de basemap propio (se monta sobre MapLibre como capa custom).
- Carga por HTTP range: header → directorio → teselas visibles. Cache en OPFS para offline.
- Benchmark obligatorio en CI: 100k segmentos con estado animado a 60fps en un móvil de gama
  media (usar trazas de Chrome DevTools; umbral configurable).

**Criterio de hecho**: demo `site/demo/` que carga un `.trama`, reproduce 24h de estado con
scrub fluido y hace un fly-through. Este es el material del lanzamiento público.

---

## Fase 5 — Plugin EPANET + playground (2-3 semanas)

### [IA]
- `solvers/epanet/`: wrapper del toolkit OWA-EPANET compilado a WASM (partir de epanet-js-toolkit
  si la licencia MIT lo permite; no reinventar el solver). Implementa el contrato v0: lee grafo
  + props del `.trama`, escribe canales `pressure`/`flow` como serie temporal → directo al ring
  buffer del engine.
- Playground en `site/demo/`: sube GeoJSON/.inp/CSV → compila **en el navegador** (compilador
  portado a WASM con Pyodide o reimplementación TS mínima — [DECISIÓN]) → elige solver →
  resultado con scrub y fly-through. Sin registro, sin subir datos a ningún servidor. Datasets
  de ejemplo precargados (Net3 de EPANET + una red viaria).

**Criterio de hecho**: un desconocido llega a la web, sube su .inp y ve su red simulada en
<60 segundos. Ese es el momento del lanzamiento en Hacker News / comunidad MapLibre.

---

## Reglas permanentes para la IA

1. **BSL 1.1 en todo el core**; cabecera de licencia en cada archivo fuente.
2. **La espec manda**: si el código necesita algo que la espec no cubre, se cambia la espec
   primero (PR separado), nunca se improvisa formato.
3. **Núcleo agnóstico**: prohibido cualquier concepto de dominio (tubería, presión, carretera)
   fuera de `solvers/`. El núcleo conoce: nodos, aristas, props tipadas, canales de estado.
4. **Cada [DECISIÓN] se presenta con 2-3 opciones y una recomendación** antes de implementar.
5. **Commits convencionales**, PRs pequeños, inglés en código y docs técnicas (audiencia
   internacional), español permitido en discusiones.
6. **No optimizar prematuramente** nada que no esté en un criterio de hecho medible.
7. Al final de cada fase: actualizar README con el estado real y anotar en `docs/DECISIONS.md`
   (ADR corto) cada decisión tomada y por qué.
