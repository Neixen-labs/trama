// SPDX-License-Identifier: LicenseRef-BSL-1.1
/** Client for the server runtime of docs/SOLVER_CONTRACT.md section 6. */

export const CONTRACT_VERSION = "0.1.0";

export type SolveRequest = Readonly<{
  tramaUrl: string;
  params?: Readonly<Record<string, unknown>>;
  t0Seconds: number;
  t1Seconds: number;
  sha256?: string;
}>;

export type SolveError = Readonly<{ code: string; message: string }>;

export class SolverFailed extends Error {
  readonly code: string;

  constructor(failure: SolveError) {
    super(`${failure.code}: ${failure.message}`);
    this.name = "SolverFailed";
    this.code = failure.code;
  }
}

/**
 * Streams a solver's deltas, yielding each event's decoded payload.
 *
 * A stream that ends without `complete` throws: section 6 requires a client to treat it as
 * failed, and silently accepting a truncated run would show a partial solve as a finished one.
 */
export async function* solveDeltas(
  endpoint: string,
  request: SolveRequest,
  fetchImpl: typeof fetch = fetch,
): AsyncGenerator<Uint8Array> {
  const response = await fetchImpl(endpoint, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      contract_version: CONTRACT_VERSION,
      trama: request.sha256 === undefined ? { url: request.tramaUrl } : { url: request.tramaUrl, sha256: request.sha256 },
      params: request.params ?? {},
      t0_seconds: request.t0Seconds,
      t1_seconds: request.t1Seconds,
    }),
  });
  if (response.body === null) throw new SolverFailed({ code: "internal_error", message: "the solver sent no body" });

  let completed = false;
  for await (const event of readEvents(response.body)) {
    if (event.name === "error") throw new SolverFailed(parseError(event.data));
    if (event.name === "complete") {
      completed = true;
      continue;
    }
    if (event.name === "delta") yield decodeBase64(event.data);
  }
  if (!completed) throw new SolverFailed({ code: "execution_failed", message: "stream ended without complete" });
}

type ServerEvent = Readonly<{ name: string; data: string }>;

async function* readEvents(body: ReadableStream<Uint8Array>): AsyncGenerator<ServerEvent> {
  const decoder = new TextDecoder();
  const reader = body.getReader();
  let buffered = "";
  for (;;) {
    const { done, value } = await reader.read();
    buffered += value === undefined ? "" : decoder.decode(value, { stream: true });
    // An event ends at a blank line, and a chunk boundary may fall anywhere, so only whole
    // events are taken and the remainder waits for more bytes.
    let boundary = buffered.indexOf("\n\n");
    while (boundary !== -1) {
      const event = parseEvent(buffered.slice(0, boundary));
      if (event !== null) yield event;
      buffered = buffered.slice(boundary + 2);
      boundary = buffered.indexOf("\n\n");
    }
    if (done) return;
  }
}

function parseEvent(block: string): ServerEvent | null {
  let name = "";
  const data: string[] = [];
  for (const line of block.split("\n")) {
    if (line.startsWith("event:")) name = line.slice("event:".length).trim();
    else if (line.startsWith("data:")) data.push(line.slice("data:".length).trim());
  }
  return name === "" ? null : { name, data: data.join("") };
}

function parseError(data: string): SolveError {
  try {
    const parsed = JSON.parse(data) as Partial<SolveError>;
    return { code: parsed.code ?? "internal_error", message: parsed.message ?? data };
  } catch {
    return { code: "internal_error", message: data };
  }
}

function decodeBase64(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}
