// SPDX-License-Identifier: LicenseRef-BSL-1.1
import type { StateRing } from "./state.js";

export type StateTexture = Readonly<{
  texture: WebGLTexture;
  /** Re-uploads the ring's current texels. Call after applying deltas. */
  update(ring: StateRing): void;
  dispose(): void;
}>;

/**
 * Uploads the ring as a single-channel float texture: column = entity index, row = (slot, channel).
 *
 * Filtering is NEAREST on purpose. Blending between slots happens in the shader with two explicit
 * fetches, so the interpolation is the one the channel declares rather than one the sampler picks,
 * and R32F sampling never needs OES_texture_float_linear.
 */
export function createStateTexture(gl: WebGL2RenderingContext, ring: StateRing): StateTexture {
  const texture = gl.createTexture();
  const upload = (source: StateRing) => {
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, source.width, source.height, 0, gl.RED, gl.FLOAT, source.texels);
  };

  gl.bindTexture(gl.TEXTURE_2D, texture);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  upload(ring);

  return {
    texture,
    update: upload,
    dispose() {
      gl.deleteTexture(texture);
    },
  };
}
