# TRAMA Solver Contract

**Contract version:** 0.1.0
**Status:** Draft
**Normative language:** The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHOULD**, **SHOULD NOT**, and **MAY** are to be interpreted as described in RFC 2119.

A TRAMA solver reads a graph and typed properties declared by a `.trama` file, then writes declared state channels. The core remains domain-agnostic: a solver MAY interpret properties, but the core does not.

A solver MAY run locally as WASM/WASI, remotely over HTTP, or support both. Both runtimes use the same state-delta bytes. A client MUST NOT need to know which runtime produced a result.

## 1. Compatibility

A solver declares the contract versions it supports. A host MUST reject a solver when no compatible contract version exists, when its manifest requirements are absent from the input file, or when it attempts to emit an undeclared channel.

Time `t` is an `f32` count of seconds from the run's declared origin. The origin is supplied by the caller; it is not an absolute timestamp.

## 2. Manifest: `solver.toml`

Every solver package MUST include one UTF-8 TOML manifest.

```toml
id = "example-network"
version = "0.1.0"
license = "Apache-2.0"
contract_versions = ["0.1.0"]
runtimes = ["wasm", "server"]

[inputs]
nodes = true
edges = true
edge_properties = ["edge_weight"]
node_properties = ["node_weight"]

[[outputs]]
channel = "edge_score"
entity_kind = "edge"
unit = "1"

[[outputs]]
channel = "node_score"
entity_kind = "node"
unit = "1"

[params]
schema = {"type" = "object", "properties" = {"iterations" = {"type" = "integer", "minimum" = 1, "default" = 100}}, "additionalProperties" = false}
```

### Required fields

- `id`: lowercase ASCII letters, digits, and `-`; globally unique within a registry.
- `version`: semantic version.
- `license`: SPDX license expression.
- `contract_versions`: non-empty supported contract-version list.
- `runtimes`: one or both of `wasm` and `server`.
- `inputs`: required graph entity kinds and property keys. Property keys are strings defined in `PROP`; they do not imply a core domain model.
- `outputs`: one or more channels that the solver may write. Each output MUST match an `STCH` declaration by name, entity kind, and unit.
- `params.schema`: an inline JSON Schema 2020-12 object. The caller validates parameters before execution.

A manifest MUST NOT claim filesystem, network, GPU, or clock access. Those capabilities are denied by default.

## 3. Input validation

Before execution, the host MUST:

1. validate the manifest syntax and contract version;
2. validate caller parameters against `params.schema`;
3. confirm each required property exists for its declared entity kind;
4. resolve every output channel name to exactly one `STCH` declaration;
5. pass only the required graph/property view to a sandboxed solver when practical.

A solver MUST treat missing, null, non-finite, and out-of-range input values as a validation error unless its manifest and parameter schema explicitly define a fallback. A solver MUST NOT write a delta for an entity ID or channel outside the input contract.

## 4. State delta

A state delta is exactly 18 bytes, packed with no alignment padding:

```text
Offset  Type  Name
0x00    u64   entity_id
0x08    u16   channel_id
0x0a    f32   t_seconds
0x0e    f32   value
```

All fields are little-endian. `entity_id` identifies a node or edge; `channel_id` resolves through `STCH`. `t_seconds` and `value` MUST be finite. A delta stream is a concatenation of complete 18-byte records; its byte length MUST be divisible by 18.

The host validates IDs, channel applicability, finite values, and declared ranges before updating its temporal ring buffer. A range violation is an error unless the associated `STCH` channel has no declared range.

## 5. WASM/WASI runtime

### 5.1 Sandbox

The host runs WASM with WASI Preview 1 compatibility in v0. It MUST deny network access and system-clock access. It MUST provide no preopened directories by default. A host MAY provide a read-only, explicitly selected input file only when the invocation requests it.

The solver uses one linear memory. The host copies the selected decoded graph view and canonical UTF-8 JSON parameters into that memory once for `init`; the solver MUST NOT mutate the graph bytes. v0 does not promise cross-runtime shared memory: “zero-copy” means the solver reads the host-provided graph view directly rather than reparsing source data or receiving per-step copies.

### 5.2 Required exports

A WASM solver MUST export these functions:

```text
trama_abi_version() -> u32
trama_alloc(bytes: u32) -> u32
trama_free(ptr: u32, bytes: u32) -> void
init(graph_ptr: u32, graph_len: u32, params_ptr: u32, params_len: u32) -> i32
step(t_seconds: f32, result_ptr: u32) -> i32
run(t0_seconds: f32, t1_seconds: f32, result_ptr: u32) -> i32
```

`trama_abi_version()` returns `0x00010000` for contract `0.1.x`. `trama_alloc` returns a non-zero aligned pointer or zero on allocation failure. The host allocates and writes graph and parameter bytes using this export.

`result_ptr` points to writable 8 bytes supplied by the host:

```text
SolverResult
  u32 delta_ptr
  u32 delta_bytes
```

On success, `step` emits all deltas for the requested time and `run` emits an ordered series covering the inclusive interval. The solver allocates `delta_ptr` with `trama_alloc`; the host reads `delta_bytes`, validates it, then calls `trama_free(delta_ptr, delta_bytes)`.

Return values are:

```text
 0  success
 1  invalid graph or parameters
 2  solver convergence or execution failure
 3  output buffer or allocation failure
 4  unsupported invocation
```

Any non-zero result invalidates `SolverResult`; the host MUST NOT read it. A solver MUST NOT retain graph or parameter pointers after the host calls `trama_free`.

This descriptor replaces a bare returned pointer: a pointer without a length is not safe or interoperable.

## 6. Server runtime

A server solver exposes `POST /solve` and returns Server-Sent Events. The request body is JSON:

```json
{
  "contract_version": "0.1.0",
  "trama": {
    "url": "https://example.invalid/network.trama",
    "sha256": "optional lowercase hexadecimal SHA-256"
  },
  "params": {"iterations": 100},
  "t0_seconds": 0,
  "t1_seconds": 86400
}
```

`trama.url` MUST be an absolute HTTPS URL. The server MUST validate the URL against its access policy before fetching it and SHOULD use range requests. If `sha256` is supplied, the server MUST verify it before solving. Authentication, authorization, and URL allowlists are deployment concerns but MUST be documented by a server implementation.

The response has `Content-Type: text/event-stream`. Events are:

```text
event: ready
data: {"contract_version":"0.1.0","solver_id":"example-flow"}

event: delta
data: AQID...base64...

event: complete
data: {"delta_count":42}
```

A `delta` event's decoded Base64 payload is one or more concatenated 18-byte state deltas, identical to the WASM output. It MUST NOT contain JSON-encoded values. The server MAY batch multiple deltas in one event.

On failure, the server sends exactly one terminal event and closes the stream:

```text
event: error
data: {"code":"invalid_input","message":"required property capacity is absent"}
```

Error codes are `invalid_request`, `unsupported_contract`, `invalid_input`, `fetch_failed`, `execution_failed`, and `internal_error`. Clients MUST treat any stream ending without `complete` as failed.

## 7. Determinism and ordering

Given identical graph bytes, canonical parameter JSON, contract version, and invocation interval, a deterministic solver SHOULD emit the same ordered delta bytes. Deltas MUST be sorted by `t_seconds`, then `channel_id`, then unsigned `entity_id`. If two deltas have the same `(entity_id, channel_id, t_seconds)`, the stream is invalid.

Solvers MAY be non-deterministic only when their manifest documents the source of non-determinism and the caller explicitly enables it through parameters.

## 8. Example lifecycle

1. The host reads `solver.toml`, validates the requested version and parameters, and matches requirements against `GRPH`, `PROP`, and `STCH`.
2. For WASM, it copies the graph view and canonical parameter JSON into module memory, calls `init`, then `step` or `run`.
3. For a server, it sends the `.trama` HTTPS reference and follows `delta` SSE events.
4. In both cases, it validates each 18-byte delta and writes it into the same GPU temporal ring buffer.

## 9. Versioning

The contract follows semantic versioning. A major version may change ABI signatures, manifest meaning, or delta bytes. A minor version may add optional fields and capabilities. A patch version clarifies behavior without changing the ABI or wire format.

A solver MUST declare every supported contract version. A host MAY ignore unknown optional manifest fields but MUST reject unknown required fields and all incompatible major versions.
