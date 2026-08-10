// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import test from "node:test";

import { createLineRenderer, type LineStyle } from "../src/line-renderer.js";
import { buildLineInstances } from "../src/lines.js";

type Call = { readonly name: string; readonly args: readonly unknown[] };

/** Records every call so the tests can assert the wiring a headless run cannot draw. */
function recordingContext(overrides: Record<string, unknown> = {}) {
  const calls: Call[] = [];
  const constants = {
    ARRAY_BUFFER: 0x8892,
    DYNAMIC_DRAW: 0x88e8,
    UNSIGNED_SHORT: 0x1403,
    UNSIGNED_INT: 0x1405,
    TRIANGLE_STRIP: 0x0005,
    TEXTURE0: 0x84c0,
    TEXTURE_2D: 0x0de1,
    R32F: 0x822e,
    RED: 0x1903,
    FLOAT: 0x1406,
    NEAREST: 0x2600,
    TEXTURE_MIN_FILTER: 0x2801,
    TEXTURE_MAG_FILTER: 0x2800,
    TEXTURE_WRAP_S: 0x2802,
    TEXTURE_WRAP_T: 0x2803,
    CLAMP_TO_EDGE: 0x812f,
    VERTEX_SHADER: 0x8b31,
    FRAGMENT_SHADER: 0x8b30,
    COMPILE_STATUS: 0x8b81,
    LINK_STATUS: 0x8b82,
  };
  const record =
    (name: string, result?: unknown) =>
    (...args: unknown[]) => {
      calls.push({ name, args });
      return result;
    };
  const gl = {
    ...constants,
    createProgram: record("createProgram", "program"),
    createShader: record("createShader", "shader"),
    createBuffer: record("createBuffer", "buffer"),
    createVertexArray: record("createVertexArray", "vao"),
    shaderSource: record("shaderSource"),
    compileShader: record("compileShader"),
    attachShader: record("attachShader"),
    linkProgram: record("linkProgram"),
    getShaderParameter: record("getShaderParameter", true),
    getProgramParameter: record("getProgramParameter", true),
    getShaderInfoLog: record("getShaderInfoLog", ""),
    getProgramInfoLog: record("getProgramInfoLog", ""),
    getAttribLocation: (_program: unknown, name: string) => ({ a_start: 0, a_end: 1, a_edge_index: 2 })[name] ?? -1,
    getUniformLocation: (_program: unknown, name: string) => name,
    getError: () => 0,
    useProgram: record("useProgram"),
    bindVertexArray: record("bindVertexArray"),
    bindBuffer: record("bindBuffer"),
    bufferData: record("bufferData"),
    enableVertexAttribArray: record("enableVertexAttribArray"),
    vertexAttribPointer: record("vertexAttribPointer"),
    vertexAttribIPointer: record("vertexAttribIPointer"),
    vertexAttribDivisor: record("vertexAttribDivisor"),
    uniformMatrix4fv: record("uniformMatrix4fv"),
    uniform1i: record("uniform1i"),
    uniform2f: record("uniform2f"),
    activeTexture: record("activeTexture"),
    bindTexture: record("bindTexture"),
    createTexture: record("createTexture", "texture"),
    texImage2D: record("texImage2D"),
    texParameteri: record("texParameteri"),
    deleteTexture: record("deleteTexture"),
    uniform1f: record("uniform1f"),
    uniform4f: record("uniform4f"),
    drawArraysInstanced: record("drawArraysInstanced"),
    deleteProgram: record("deleteProgram"),
    deleteBuffer: record("deleteBuffer"),
    deleteVertexArray: record("deleteVertexArray"),
    deleteShader: record("deleteShader"),
    ...overrides,
  };
  return { gl: gl as unknown as WebGL2RenderingContext, calls, constants };
}

const style: LineStyle = {
  matrix: new Float32Array(16),
  resolutionPixels: [800, 600],
  widthPixels: 4,
  color: [1, 0, 0, 1],
};

const twoSegments = buildLineInstances({
  meshVertexCount: 0,
  meshIndexCount: 0,
  paths: [{ edgeIndex: 3, vertices: Uint16Array.from([0, 0, 100, 100, 200, 200]) }],
});

function callsNamed(calls: readonly Call[], name: string) {
  return calls.filter((call) => call.name === name);
}

test("compiles both shaders and links the program once", () => {
  const { gl, calls } = recordingContext();

  createLineRenderer(gl);

  assert.equal(callsNamed(calls, "compileShader").length, 2);
  assert.equal(callsNamed(calls, "attachShader").length, 2);
  assert.equal(callsNamed(calls, "linkProgram").length, 1);
});

test("reports a shader that fails to compile", () => {
  const { gl } = recordingContext({ getShaderParameter: () => false, getShaderInfoLog: () => "syntax error" });

  assert.throws(() => createLineRenderer(gl), /line shader failed to compile: syntax error/);
});

test("reports a program that fails to link", () => {
  const { gl } = recordingContext({ getProgramParameter: () => false, getProgramInfoLog: () => "no such varying" });

  assert.throws(() => createLineRenderer(gl), /line program failed to link: no such varying/);
});

test("wires every attribute to the instance layout", () => {
  const { gl, calls, constants } = recordingContext();

  createLineRenderer(gl).draw(twoSegments, style);

  assert.deepEqual(
    callsNamed(calls, "vertexAttribPointer").map((call) => call.args),
    [
      [0, 2, constants.UNSIGNED_SHORT, true, twoSegments.strideBytes, twoSegments.layout.start],
      [1, 2, constants.UNSIGNED_SHORT, true, twoSegments.strideBytes, twoSegments.layout.end],
    ],
  );
  assert.deepEqual(callsNamed(calls, "vertexAttribIPointer")[0]?.args, [
    2,
    1,
    constants.UNSIGNED_INT,
    twoSegments.strideBytes,
    twoSegments.layout.edgeIndex,
  ]);
});

test("advances every attribute once per instance, not once per vertex", () => {
  const { gl, calls } = recordingContext();

  createLineRenderer(gl).draw(twoSegments, style);

  assert.deepEqual(
    callsNamed(calls, "vertexAttribDivisor").map((call) => call.args),
    [
      [0, 1],
      [1, 1],
      [2, 1],
    ],
  );
});

test("draws one four-vertex strip per instance", () => {
  const { gl, calls, constants } = recordingContext();

  createLineRenderer(gl).draw(twoSegments, style);

  assert.deepEqual(callsNamed(calls, "drawArraysInstanced")[0]?.args, [
    constants.TRIANGLE_STRIP,
    0,
    4,
    twoSegments.count,
  ]);
  assert.equal(callsNamed(calls, "bufferData")[0]?.args[1], twoSegments.buffer);
});

test("uploads the style the host asked for", () => {
  const { gl, calls } = recordingContext();

  createLineRenderer(gl).draw(twoSegments, style);

  assert.equal(callsNamed(calls, "uniformMatrix4fv")[0]?.args[2], style.matrix);
  assert.deepEqual(callsNamed(calls, "uniform2f")[0]?.args.slice(1), [800, 600]);
  assert.deepEqual(callsNamed(calls, "uniform1f")[0]?.args.slice(1), [4]);
  assert.deepEqual(callsNamed(calls, "uniform4f")[0]?.args.slice(1), [1, 0, 0, 1]);
});

test("skips the draw call entirely for an empty tile", () => {
  const { gl, calls } = recordingContext();
  const empty = buildLineInstances({ meshVertexCount: 0, meshIndexCount: 0, paths: [] });

  createLineRenderer(gl).draw(empty, style);

  assert.equal(callsNamed(calls, "drawArraysInstanced").length, 0);
  assert.equal(callsNamed(calls, "bufferData").length, 0);
});

test("releases every GL object it created", () => {
  const { gl, calls } = recordingContext();

  createLineRenderer(gl).dispose();

  assert.equal(callsNamed(calls, "deleteProgram").length, 1);
  assert.equal(callsNamed(calls, "deleteBuffer").length, 1);
  assert.equal(callsNamed(calls, "deleteVertexArray").length, 1);
});


test("tells the shader no state is bound when the style has none", () => {
  const { gl, calls } = recordingContext();

  createLineRenderer(gl).draw(twoSegments, style);

  assert.deepEqual(callsNamed(calls, "uniform1i")[0]?.args, ["u_row_a", -1]);
  assert.equal(callsNamed(calls, "bindTexture").length, 0);
});

test("binds the state texture and the rows to blend", () => {
  const { gl, calls, constants } = recordingContext();

  createLineRenderer(gl).draw(twoSegments, {
    ...style,
    state: {
      texture: "state-texture" as unknown as WebGLTexture,
      rows: { rowA: 2, rowB: 3, mix: 0.25 },
      range: [0, 100],
      highColor: [0, 0, 1, 1],
    },
  });

  assert.deepEqual(callsNamed(calls, "activeTexture")[0]?.args, [constants.TEXTURE0]);
  assert.deepEqual(callsNamed(calls, "bindTexture")[0]?.args, [constants.TEXTURE_2D, "state-texture"]);
  assert.deepEqual(
    callsNamed(calls, "uniform1i").map((call) => call.args),
    [
      ["u_state", 0],
      ["u_row_a", 2],
      ["u_row_b", 3],
    ],
  );
  assert.deepEqual(callsNamed(calls, "uniform1f").at(-1)?.args, ["u_mix", 0.25]);
  assert.deepEqual(callsNamed(calls, "uniform2f").at(-1)?.args, ["u_range", 0, 100]);
});
