// SPDX-License-Identifier: LicenseRef-BSL-1.1
import type { LineInstances } from "./lines.js";

/** Colours a line by its state value; omit it and every line draws in `color`. */
export type StateStyle = Readonly<{
  texture: WebGLTexture;
  /** The two rows to blend and the factor between them, from `StateRing.sampleRows`. */
  rows: Readonly<{ rowA: number; rowB: number; mix: number }>;
  /** Value range mapped across the ramp, normally the channel's declared range. */
  range: readonly [number, number];
  highColor: readonly [number, number, number, number];
}>;

export type LineStyle = Readonly<{
  /** Tile-normalized [0,1] coordinates to clip space, as a MapLibre custom layer receives. */
  matrix: Float32Array;
  resolutionPixels: readonly [number, number];
  widthPixels: number;
  color: readonly [number, number, number, number];
  state?: StateStyle;
}>;

export type LineRenderer = Readonly<{
  draw(instances: LineInstances, style: LineStyle): void;
  dispose(): void;
}>;

// Each instance becomes a 4-vertex strip. The normal is computed in screen space, so width
// stays constant in pixels at any zoom — what SPEC 3.3 traded the pre-baked mesh for.
const VERTEX_SHADER = `#version 300 es
precision highp float;
in vec2 a_start;
in vec2 a_end;
in uint a_edge_index;
uniform mat4 u_matrix;
uniform vec2 u_resolution;
uniform float u_width;
flat out uint v_edge_index;
void main() {
  vec2 corner = vec2(float(gl_VertexID >> 1), float(gl_VertexID & 1));
  vec4 clip_start = u_matrix * vec4(a_start, 0.0, 1.0);
  vec4 clip_end = u_matrix * vec4(a_end, 0.0, 1.0);
  vec2 screen_direction = clip_end.xy / clip_end.w * u_resolution - clip_start.xy / clip_start.w * u_resolution;
  // A zero-length segment has no direction; normalizing it would push the whole strip to NaN.
  vec2 normal = length(screen_direction) > 0.0
    ? normalize(vec2(-screen_direction.y, screen_direction.x))
    : vec2(0.0);
  vec4 clip = mix(clip_start, clip_end, corner.x);
  clip.xy += normal * (corner.y * 2.0 - 1.0) * u_width / u_resolution * clip.w;
  gl_Position = clip;
  v_edge_index = a_edge_index;
}`;

// Two explicit fetches rather than a filtered sample: the blend is the one the channel declares,
// and R32F never needs OES_texture_float_linear. A negative u_row_a means no state is bound.
const FRAGMENT_SHADER = `#version 300 es
precision highp float;
flat in uint v_edge_index;
uniform vec4 u_color;
uniform vec4 u_high_color;
uniform highp sampler2D u_state;
uniform int u_row_a;
uniform int u_row_b;
uniform float u_mix;
uniform vec2 u_range;
out vec4 fragment_color;
void main() {
  if (u_row_a < 0) {
    fragment_color = u_color;
    return;
  }
  int column = int(v_edge_index);
  float value = mix(
    texelFetch(u_state, ivec2(column, u_row_a), 0).r,
    texelFetch(u_state, ivec2(column, u_row_b), 0).r,
    u_mix);
  float span = u_range.y - u_range.x;
  float normalized = span > 0.0 ? clamp((value - u_range.x) / span, 0.0, 1.0) : 0.0;
  fragment_color = mix(u_color, u_high_color, normalized);
}`;

const VERTICES_PER_INSTANCE = 4;

export function createLineRenderer(gl: WebGL2RenderingContext): LineRenderer {
  const program = link(gl);
  const buffer = gl.createBuffer();
  const vertexArray = gl.createVertexArray();
  const attributes = {
    start: gl.getAttribLocation(program, "a_start"),
    end: gl.getAttribLocation(program, "a_end"),
    edgeIndex: gl.getAttribLocation(program, "a_edge_index"),
  };
  const uniforms = {
    matrix: gl.getUniformLocation(program, "u_matrix"),
    resolution: gl.getUniformLocation(program, "u_resolution"),
    width: gl.getUniformLocation(program, "u_width"),
    color: gl.getUniformLocation(program, "u_color"),
    highColor: gl.getUniformLocation(program, "u_high_color"),
    state: gl.getUniformLocation(program, "u_state"),
    rowA: gl.getUniformLocation(program, "u_row_a"),
    rowB: gl.getUniformLocation(program, "u_row_b"),
    mix: gl.getUniformLocation(program, "u_mix"),
    range: gl.getUniformLocation(program, "u_range"),
  };

  return {
    draw(instances, style) {
      if (instances.count === 0) return;
      gl.useProgram(program);
      gl.bindVertexArray(vertexArray);
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
      gl.bufferData(gl.ARRAY_BUFFER, instances.buffer, gl.DYNAMIC_DRAW);

      // normalized = true turns the tile-local u16 of SPEC 3.1 into the [0,1] the matrix expects.
      for (const [location, offset] of [
        [attributes.start, instances.layout.start],
        [attributes.end, instances.layout.end],
      ] as const) {
        gl.enableVertexAttribArray(location);
        gl.vertexAttribPointer(location, 2, gl.UNSIGNED_SHORT, true, instances.strideBytes, offset);
        gl.vertexAttribDivisor(location, 1);
      }
      gl.enableVertexAttribArray(attributes.edgeIndex);
      gl.vertexAttribIPointer(attributes.edgeIndex, 1, gl.UNSIGNED_INT, instances.strideBytes, instances.layout.edgeIndex);
      gl.vertexAttribDivisor(attributes.edgeIndex, 1);

      gl.uniformMatrix4fv(uniforms.matrix, false, style.matrix);
      gl.uniform2f(uniforms.resolution, style.resolutionPixels[0], style.resolutionPixels[1]);
      gl.uniform1f(uniforms.width, style.widthPixels);
      gl.uniform4f(uniforms.color, style.color[0], style.color[1], style.color[2], style.color[3]);
      bindState(gl, uniforms, style.state);

      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, VERTICES_PER_INSTANCE, instances.count);
      gl.bindVertexArray(null);
    },
    dispose() {
      gl.deleteProgram(program);
      gl.deleteBuffer(buffer);
      gl.deleteVertexArray(vertexArray);
    },
  };
}

type Uniforms = Record<"state" | "rowA" | "rowB" | "mix" | "range" | "highColor", WebGLUniformLocation | null>;

/** A negative row is how the shader is told no state is bound; the flat colour then wins. */
function bindState(gl: WebGL2RenderingContext, uniforms: Uniforms, state: StateStyle | undefined): void {
  if (state === undefined) {
    gl.uniform1i(uniforms.rowA, -1);
    return;
  }
  gl.activeTexture(gl.TEXTURE0);
  gl.bindTexture(gl.TEXTURE_2D, state.texture);
  gl.uniform1i(uniforms.state, 0);
  gl.uniform1i(uniforms.rowA, state.rows.rowA);
  gl.uniform1i(uniforms.rowB, state.rows.rowB);
  gl.uniform1f(uniforms.mix, state.rows.mix);
  gl.uniform2f(uniforms.range, state.range[0], state.range[1]);
  gl.uniform4f(uniforms.highColor, state.highColor[0], state.highColor[1], state.highColor[2], state.highColor[3]);
}

function link(gl: WebGL2RenderingContext): WebGLProgram {
  const program = gl.createProgram();
  for (const [type, source] of [
    [gl.VERTEX_SHADER, VERTEX_SHADER],
    [gl.FRAGMENT_SHADER, FRAGMENT_SHADER],
  ] as const) {
    gl.attachShader(program, compile(gl, type, source));
  }
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    const log = gl.getProgramInfoLog(program);
    gl.deleteProgram(program);
    throw new Error(`line program failed to link: ${log ?? "no log"}`);
  }
  return program;
}

function compile(gl: WebGL2RenderingContext, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (shader === null) throw new Error("could not create a shader");
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(shader);
    gl.deleteShader(shader);
    throw new Error(`line shader failed to compile: ${log ?? "no log"}`);
  }
  return shader;
}
