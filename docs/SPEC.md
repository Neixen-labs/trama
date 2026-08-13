# TRAMA File Format Specification

**Specification version:** 0.4.0
**Status:** Draft
**Normative language:** The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHOULD**, **SHOULD NOT**, and **MAY** are to be interpreted as described in RFC 2119.

TRAMA is a domain-agnostic, single-file binary format for network maps. A reader knows nodes, edges, typed properties, render geometry, and declared state channels. It MUST NOT need domain concepts to decode a file.

v0 has four required section kinds and one optional one:

- `GEOM`: independently fetchable, pre-tessellated render geometry tiles;
- `GRPH`: stable graph identities, topology, CSR adjacency, and geometry references;
- `PROP`: typed node and edge properties with a global key dictionary;
- `STCH`: state-channel declarations;
- `XTRA`: optional opaque payloads belonging to a named owner, which the core stores and never interprets.

A TRAMA file contains no runtime state samples or solver results.

## 1. Conventions

- All fields are little-endian.
- Primitive types are fixed-width: `u8`, `u16`, `u32`, `u64`, `i64`, `f32`, and `f64`.
- A string is UTF-8, length-prefixed by a `u32` byte count, and has no terminator.
- A FourCC is four ASCII bytes; `GEOM` is `47 45 4d 4d`.
- File offsets are absolute `u64` byte offsets from byte zero.
- Reserved fields MUST be zero when written and ignored when read.
- Readers MUST bounds-check every offset, count, multiplication, decoded byte length, and reference before allocating or dereferencing memory.

### 1.1 Spatial reference and tile scheme

v0 render geometry uses the Web Mercator slippy-map tile matrix, `EPSG:3857`, with `z/x/y` keys. This lets a range reader select visible geometry from the directory without fetching a tile payload. Exact source or engineering coordinates MAY be retained as typed properties.

A later major format version MAY add another coordinate reference system or tile matrix. A v0 reader MUST reject such an extension.

## 2. Container

```text
+-------------------+
| 64-byte header    |
+-------------------+
| section directory | uncompressed; section_count × 64 bytes
+-------------------+
| aligned sections  | each section record is one independent zstd frame
+-------------------+
```

A range reader fetches the header, then the directory, then only the visible `GEOM` records. It fetches `GRPH`, `PROP`, and `STCH` only when graph, property, or solver metadata is required.

### 2.1 Header (64 bytes)

| Offset | Type | Name | Meaning |
|---:|---|---|---|
| `0x00` | `char[8]` | `magic` | `TRAMA\0\0\0` |
| `0x08` | `u16 × 3` | `format_version` | major, minor, patch |
| `0x0e` | `u16 × 3` | `minimum_reader_version` | major, minor, patch |
| `0x14` | `u32` | `header_bytes` | MUST be `64` in v0 |
| `0x18` | `u64` | `section_directory_offset` | normally `64` |
| `0x20` | `u32` | `section_count` | directory record count |
| `0x24` | `u32` | `container_flags` | zero in v0 |
| `0x28` | `u64` | `file_bytes` | complete object length |
| `0x30` | `u8[16]` | `file_uuid` | deterministic file identity, not an entity identity |

A reader MUST reject a file when its supported version is lower than `minimum_reader_version`, when `header_bytes != 64`, or when a known object length disagrees with `file_bytes`. A writer MUST set `file_uuid` to the first 16 bytes of SHA-256 over the decoded section payloads in directory order. This makes repeated compilation of identical logical data byte-identical.

### 2.2 Directory record (64 bytes)

| Offset | Type | Name | Meaning |
|---:|---|---|---|
| `0x00` | `FourCC` | `type` | `GEOM`, `GRPH`, `PROP`, `STCH`, or `XTRA` |
| `0x04` | `u32` | `record_flags` | bit 0: required; remaining bits zero in v0 |
| `0x08` | `u32` | `key0` | `GEOM`: zoom `z`; otherwise zero |
| `0x0c` | `u32` | `key1` | `GEOM`: tile `x`; otherwise zero |
| `0x10` | `u32` | `key2` | `GEOM`: tile `y`; otherwise zero |
| `0x14` | `u64` | `offset` | stored payload start |
| `0x1c` | `u64` | `stored_bytes` | zstd frame byte count |
| `0x24` | `u64` | `uncompressed_bytes` | decoded byte count |
| `0x2c` | `u32` | `crc32c` | CRC-32C of decoded bytes |
| `0x30` | `u16` | `codec` | `1 = zstd` |
| `0x32` | `u8` | `alignment_log2` | `12` means 4096-byte alignment |
| `0x33` | `u8` | `reserved0` | zero |
| `0x34` | `u8[12]` | `reserved1` | zero |

There MUST be exactly one `GRPH`, one `PROP`, and one `STCH` record. There MAY be zero or more `GEOM` records; each `(z, x, y)` tuple MUST be unique. There MAY be zero or more `XTRA` records under the rules of section 7. All v0 records use `codec = 1`; the directory is never compressed. Writers SHOULD align payloads to 4096 bytes.

Readers MUST verify decoded length and CRC-32C. v0 uses no shared zstd dictionary, so every directory record is independently decodable.

### 2.3 Illustrative hexdump

This structural example has format `0.1.0`, a 64-byte header, and its first directory record is `GEOM z=12/x=345/y=678`:

```text
00000000  54 52 41 4d 41 00 00 00 00 00 01 00 00 00 00 00
00000010  01 00 00 00 40 00 00 00 40 00 00 00 00 00 00 00
00000020  04 00 00 00 00 00 00 00 a0 02 00 00 00 00 00 00
00000030  00 11 22 33 44 55 66 77 88 99 aa bb cc dd ee ff
00000040  47 45 4f 4d 01 00 00 00 0c 00 00 00 59 01 00 00
00000050  a6 02 00 00 40 01 00 00 00 00 00 00 80 00 00 00
00000060  00 00 00 00 20 01 00 00 00 00 00 00 a1 b2 c3 d4
00000070  01 00 06 00 00 00 00 00 00 00 00 00 00 00 00 00
```

## 3. `GEOM`: tile-local geometry

Each `GEOM` record is one `EPSG:3857` tile. It contains ordered centerline paths for traversal and, for geometry that needs it, a triangle mesh for rendering.

### 3.1 Quantization

Positions are normalized `u16` coordinates relative to the tile extent:

```text
extent = 65535
qx = round((x - tile_min_x) / (tile_max_x - tile_min_x) * extent)
qy = round((tile_max_y - y) / (tile_max_y - tile_min_y) * extent)
```

`round` is round-half-to-even: a value exactly halfway between two integers MUST take the even one, as IEEE 754 does by default. Most languages disagree here — `f64::round`, C's `round` and `Math.round` all move away from the tie rather than towards the even neighbour — so a writer on those MUST say so explicitly. A vertex landing on a half-step is rare, but section 4.2 derives node identity from the quantized cell, so two writers rounding differently would give the same input different stable IDs.

Values are clamped to `[0, 65535]`; the inverse formula MUST be used for export. Quantized geometry is not an authoritative survey or engineering-coordinate store.

### 3.2 Decoded payload

```text
GeometryTileHeader
  u32 path_count
  u32 path_vertex_count
  u32 mesh_vertex_count
  u32 mesh_index_count
  u32 paths_offset
  u32 path_vertices_offset
  u32 mesh_vertices_offset
  u32 mesh_indices_offset

Path[path_count]
  u32 edge_index              # index in GRPH Edge[]
  u32 first_vertex
  u32 vertex_count            # at least 2
  u32 flags                   # zero in v0

PathVertex[path_vertex_count]
  u16 qx
  u16 qy

MeshVertex[mesh_vertex_count]
  u16 qx
  u16 qy
  u32 edge_index              # index in GRPH Edge[]

u32 MeshIndex[mesh_index_count]
```

Mesh indices MUST be less than `mesh_vertex_count`; their count MUST be divisible by three. A tile may contain clipped pieces of an edge. `PathVertex` order is authoritative for traversal within that tile; the mesh is a rendering derivative.

### 3.3 Line geometry carries no mesh

A `MeshVertex` states a position and an edge, not which side of a centerline it sits on. It therefore cannot describe a ribbon whose width is constant in screen space, because the offset direction is unknown to the renderer and the width is unknown until the camera is known.

A writer MUST NOT tessellate line geometry. For a tile whose paths are all lines, `mesh_vertex_count` and `mesh_index_count` MUST be zero, and `mesh_vertices_offset` and `mesh_indices_offset` MUST point at the end of the vertex array. A renderer builds the ribbon itself from consecutive `PathVertex` pairs, which keeps stroke width a camera-time and style-time decision rather than a compile-time one.

The mesh fields remain in the header for geometry that genuinely requires triangulation — a polygon interior cannot be reconstructed from a centerline. A reader MUST accept a zero mesh and MUST NOT treat it as a malformed tile.

## 4. `GRPH`: stable graph and CSR topology

Entity identity is independent of array position, tile placement, tessellation, compression, and property order.

- Node and edge IDs are unsigned `u64` and unique within their class.
- IDs MUST remain stable across recompilations when source identity is unchanged.
- IDs MUST NOT derive from array position.
- JavaScript APIs expose IDs as `bigint` or decimal strings, never `number`.
- Node and edge arrays MUST be sorted by ascending stable ID.

```text
GraphHeader
  u32 node_count
  u32 edge_count
  u32 adjacency_count
  u32 geometry_ref_count
  u32 nodes_offset
  u32 edges_offset
  u32 csr_offsets_offset
  u32 adjacency_offset
  u32 geometry_refs_offset
  u32 node_ids_offset
  u32 edge_ids_offset

Node[node_count]
  u32 property_row
  u32 flags                    # zero in v0

Edge[edge_count]
  u32 source_node_index
  u32 target_node_index
  u32 property_row
  u32 geometry_ref_start
  u32 geometry_ref_count
  u32 flags                    # bit 0: directed

NodeId, EdgeId                 # one delta block each, see 4.1

u64 CsrOffset[node_count + 1]

Adjacency[adjacency_count]
  u32 edge_index
  i8 traversal_direction       # +1 source→target; -1 target→source
  u8[3] reserved

GeometryRef[geometry_ref_count]
  u32 geometry_directory_index
  u32 path_index
  i8 direction                 # +1 stored path order; -1 reverse it
  u8[3] reserved
```

An `id` field no longer appears in either record; section 4.1 says where identity lives. `CsrOffset[0]` MUST be zero, `CsrOffset[node_count]` MUST equal `adjacency_count`, and offsets MUST be monotonic. A directed edge has one source-node entry; an undirected edge has both endpoint entries with opposite direction. Each edge references one or more geometry paths in traversal order. Every referenced directory record MUST be `GEOM` and each path's `edge_index` MUST equal the referencing edge index.

### 4.1 Identity is stored as ascending deltas

Identities do not live in the fixed-stride records. Each array's ids are a block of unsigned LEB128 varints: the first is the smallest id, and every later one is the gap to its predecessor. A block holds exactly `node_count` or `edge_count` values, and a reader decodes it once, in order, to recover the `i`th entity's id.

The ordering this relies on is already required — both arrays are sorted by ascending id — and the ordering is what makes it work. An id is the first 8 bytes of a SHA-256, so a raw array is incompressible by construction: measured on a 49,612-edge network, 597 kB of ids compress to 597 kB. Sorted, their gaps average `2^64 / n`, which is roughly six bytes rather than eight, and the same 597 kB stores in 509 kB.

An unsigned varint encodes seven bits per byte, low group first, with the high bit set on every byte but the last. Gaps are strictly positive, since ids are unique and ascending; a zero gap after the first value is a malformed section. A reader MUST reject a block whose values run past its bounds or whose count disagrees with the header.

This costs random access: the id of one entity cannot be read without decoding the block up to it. Nothing in the format needs that — a reader that wants ids wants all of them, to build the map from id to index — and the alternative was failing the size budget with eight bytes of incompressible hash per entity.

### 4.2 Node identity derived from position

A source that names its nodes decides identity by itself. When a writer has no such name and must decide whether two edge endpoints meet, it MUST compare them on the section 3.1 quantization grid, and MUST NOT compare raw coordinates for equality.

The grid cell of a projected point is:

```text
extent = 65535
qx, qy = section 3.1 quantization within the tile containing the point
cell_x = tile_x * extent + qx
cell_y = tile_y * extent + qy
```

Two endpoints are the same node when their cells are equal. Section 3.1 samples both tile edges, so `qx = extent` in one tile names the position `qx = 0` names in the next: a point on a tile boundary yields one cell whichever tile it is assigned to.

Identity is therefore fixed at the precision the file stores and no finer — about 4 cm at the equator at `z14`, and a shorter ground distance towards the poles. Two nodes closer than one cell become one node. A source needing finer topology than the geometry it ships MUST name its nodes.

Exact equality is not an acceptable substitute. Shared vertices written by different tools, or round-tripped through different precisions, routinely differ in their last digits; joining on those bits splits one node into two and disconnects the graph while its geometry still draws correctly.

## 5. `PROP`: typed properties

`PROP` has a global UTF-8 key dictionary and typed nullable columns. It carries no domain semantics.

```text
PropertyHeader
  u32 key_count
  u32 string_count
  u32 enum_count
  u32 node_column_count
  u32 edge_column_count
  u32 key_dictionary_offset
  u32 string_dictionary_offset
  u32 enum_dictionary_offset
  u32 node_columns_offset
  u32 edge_columns_offset

PropertyColumn[node_column_count + edge_column_count]
  u32 key_id
  u8 entity_kind               # 1=node, 2=edge
  u8 value_type                # 1=f64, 2=i64, 3=string, 4=bool, 5=enum
  u16 flags                    # bit 0: nullable
  u32 entity_count
  u32 presence_bitmap_offset
  u32 values_offset
```

Each dictionary is `u32 count` followed by that many length-prefixed UTF-8 strings. Keys are globally unique. Columns are ordered by entity kind then `key_id`. A presence bitmap has one bit per entity; `1` means a value is present. Present values are dense and ordered by entity-array index. Strings are `u32` indexes into the string dictionary; enums are `u32` indexes into the enum dictionary; booleans are packed bits. `f64` values MUST be finite. Absence is distinct from `false`, `0`, and an empty string.

## 6. `STCH`: state-channel declarations

`STCH` declares what solvers may write. It never contains samples, deltas, or results.

```text
StateChannelsHeader
  u32 channel_count
  u32 strings_offset
  u32 channels_offset

StateChannel[channel_count]
  u16 channel_id               # unique and non-zero
  u8 entity_kind               # 1=node, 2=edge
  u8 value_type                # MUST be 1=scalar f32 in v0
  u32 name_string_id
  u32 unit_string_id
  f32 declared_min
  f32 declared_max
  u32 flags                    # bit 0: range_present; bit 1: linear_interpolation
```

`strings_offset` points to `u32 count` followed by length-prefixed UTF-8 strings. A channel applies to exactly one entity kind. When `range_present` is set, `declared_min <= declared_max`. v0 supports scalar `f32` only, matching the GPU texture representation.

The solver delta tuple is:

```text
(entity_id: u64, channel_id: u16, t: f32, value: f32)
```

## 7. `XTRA`: opaque owner-scoped payloads

Some source formats carry information the core cannot type: a time-series pattern, a pump curve, a small control language with conditions and actions. An `XTRA` record lets a file keep that information so a domain round trip is possible, without the core acquiring a domain to understand it.

An `XTRA` payload is bytes with an owner. The core stores, compresses, checksums, and range-serves it. A core reader MUST NOT parse it and MUST NOT branch on its contents.

```text
ExtraHeader
  u32 owner_offset
  u32 owner_bytes
  u32 media_type_offset
  u32 media_type_bytes
  u32 payload_offset
  u32 payload_bytes
  u32 flags                    # zero in v0

u8 owner[owner_bytes]          # UTF-8, matches a solver.toml `id`
u8 media_type[media_type_bytes]  # UTF-8 media type naming the payload encoding
u8 payload[payload_bytes]
```

- `record_flags` bit 0 (required) MUST be zero. This is not a convention: a reader that predates `XTRA`, or does not know the owner, ignores the record through the mechanism section 9 already defines.
- `key0`, `key1`, and `key2` MUST be zero. An `XTRA` record is not tile-scoped.
- Each `(owner, media_type)` pair MUST appear at most once.
- A reader that does not recognise `owner` MUST ignore the record and MUST NOT reject the file.

### 7.1 What may not go in it

Removing every `XTRA` record from a file MUST leave a valid file that decodes, renders, and traverses identically. A writer therefore MUST NOT place in `XTRA` anything another section can express: geometry, topology, identity, typed properties, or channel declarations. `XTRA` holds what the format would otherwise lose, never what is inconvenient to encode properly.

References point one way. A payload MAY identify entities by their stable `u64` IDs; no other section may refer to an `XTRA` record. This is what makes dropping one safe, and it is the invariant to check when reviewing a change that adds to one.

## 8. Compression and integrity

Every `GEOM`, `GRPH`, `PROP`, and `STCH` directory record is one independent zstd frame. v0 permits no other codec, no mixed codecs, and no cross-record dictionary. Writers SHOULD use deterministic zstd settings. Compression level is not format-significant. A reader MUST reject malformed frames, decoded-size mismatches, checksum mismatches, and invalid references.

Each frame MUST declare its decompressed size in its own header, and that size MUST equal the directory's `uncompressed_bytes`. The directory already carries the authoritative length, so a frame disagreeing with it is corrupt either way; requiring the field costs a writer nothing and lets a reader reject a truncated frame before allocating for it. Streaming compression APIs omit the field by default, so a writer MUST use a one-shot call or pledge the size before writing.

## 9. Export and import mapping

### GeoJSON

Export creates `nodes` Point and `edges` LineString FeatureCollections. Coordinates transform from `EPSG:3857` to WGS 84. Stable IDs appear as decimal `properties["_trama_id"]` strings. Typed properties become feature properties; enum values become their declared labels. Import preserves IDs only when `_trama_id` is valid. It cannot preserve meshes, tile boundaries, CSR ordering, compression, or every nullable-type distinction.

An edge is directed when `properties["_trama_directed"]` is the JSON boolean `true`, which sets `Edge.flags` bit 0 and gives the edge one CSR entry instead of two, as section 4 requires. Any other value, and the key's absence, mean undirected; a writer MUST reject a non-boolean. Export writes the key only for directed edges, so a file with none round-trips byte for byte.

Direction is the stored vertex order, source to target. An edge has no way to express a sign, and section 4 gives it none: a source declaring the reverse direction MUST reverse the LineString. This is what keeps the core free of the domain — an input format that says a street runs against its own geometry is describing a road, and translating that into a vertex order is the producer's job, not the format's.

`_trama_id` and `_trama_directed` are the reserved keys. Neither becomes a `PROP` column, so a round trip does not grow a property the source never had, and a producer MUST NOT use either name for its own data.

### GeoPackage

Export creates `nodes` and `edges` feature layers in `EPSG:3857`, with `trama_id TEXT NOT NULL`. Typed properties map to SQLite-compatible columns and enums to text labels. Import MAY retain state-channel metadata in a metadata table, but cannot preserve mesh buffers or exact section layout.

### EPANET

Import and export of EPANET `.inp` are defined by `docs/EPANET_BOUNDARY.md` and implemented in `core/trama-epanet`, not in the core format crate. The core's obligations are these:

- Junctions, reservoirs, and tanks become nodes; pipes, pumps, and valves become edges. Per-entity scalars become `PROP` columns under opaque string keys, and the entity's EPANET name is one of them, since the rest of the file refers to entities by name.
- An `.inp` declares no coordinate reference system, so an importer MUST require the caller to state one and MUST NOT infer it from the coordinate ranges. Section 4.2 explains why an inferred spatial answer is the wrong kind of wrong: it is invisible in a rendered map.
- Everything with no entity to hang on — patterns, curves, controls, rules, options, times — goes in an `XTRA` record owned by `epanet`, under the rules of section 7. Units belong there too: `PROP` columns carry no unit, and EPANET's are set file-wide by `[OPTIONS] UNITS`.

A round trip `.inp → .trama → .inp` is verified by simulation, not by bytes: both files run through the same EPANET binary MUST agree on every node pressure and link flow, within solver tolerance, at every reported timestep. Comments, section order, and whitespace are not information about the network, and byte comparison would fail on all three while missing a dropped pattern.

### Mapbox Vector Tiles

MVT is export-only in v0. The exporter emits `nodes` and `edges` layers for selected `GEOM z/x/y` records. MVT extent is `4096`; convert a normalized coordinate with `round(q * 4096 / 65535)`. MVT is not graph-preserving: it loses CSR topology, traversal order, nullable typing, channel declarations, mesh details, and possibly `u64` fidelity.

## 10. Versioning and compatibility

Format versions follow semantic versioning:

- **Major:** incompatible binary or semantic change.
- **Minor:** backward-compatible addition; unknown optional records may be ignored.
- **Patch:** clarification or bug fix with no binary-layout change.

A writer MUST set `minimum_reader_version` to the oldest reader able to interpret every required record. Readers MUST reject unknown required records, duplicate singleton sections, invalid tile keys, non-zero reserved fields in strict mode, malformed zstd payloads, bad checksums, and invalid references. v0 has no in-place mutation model: a new dataset version is a new `.trama` file and `file_uuid`; unchanged source entities retain their stable IDs.
