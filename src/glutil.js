// Just enough WebGL plumbing for the three programs this wallpaper draws.

export function compile(gl, type, source, label) {
  const shader = gl.createShader(type);
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(`${label}: ${gl.getShaderInfoLog(shader)}`);
  }
  return shader;
}

export function program(gl, vertexSource, fragmentSource, label) {
  const prog = gl.createProgram();
  gl.attachShader(prog, compile(gl, gl.VERTEX_SHADER, vertexSource, `${label} vertex`));
  gl.attachShader(prog, compile(gl, gl.FRAGMENT_SHADER, fragmentSource, `${label} fragment`));
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    throw new Error(`${label} link: ${gl.getProgramInfoLog(prog)}`);
  }
  return prog;
}

/// Collect every uniform location up front; missing ones come back null and
/// setting a null location is a no-op, which is what we want for uniforms the
/// optimiser stripped.
export function uniforms(gl, prog, names) {
  const map = {};
  for (const name of names) map[name] = gl.getUniformLocation(prog, name);
  return map;
}

const textures = new Map();
const sizes = new Map();

/// Fetch and decode an image off the main thread. createImageBitmap does the
/// JPEG decode on a worker, so even an 8K map never blocks a frame while it
/// unpacks. Returns null if anything fails; the caller falls back to <img>.
async function decodeOffThread(url, crossOrigin) {
  if (typeof createImageBitmap !== "function") return null;
  try {
    const response = await fetch(url, crossOrigin ? { mode: "cors" } : undefined);
    if (!response.ok) return null;
    return await createImageBitmap(await response.blob());
  } catch {
    return null;
  }
}

function decodeWithImage(url, crossOrigin) {
  return new Promise((resolve) => {
    const image = new Image();
    // A remote image has to be fetched with CORS, or WebGL refuses to read it.
    if (crossOrigin) image.crossOrigin = "anonymous";
    image.onload = () => resolve(image);
    image.onerror = () => resolve(null);
    image.src = url;
  });
}

/// Load `url` into texture unit `unit`. Calling it again for the same unit
/// replaces the image in place, so a refresh never leaves the unit empty:
/// the previous image stays bound until the new one has fully decoded.
export async function loadTexture(gl, url, unit, { wrapX = true, crossOrigin = false } = {}) {
  let texture = textures.get(unit);
  if (!texture) {
    texture = gl.createTexture();
    textures.set(unit, texture);
    gl.activeTexture(gl.TEXTURE0 + unit);
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE,
      new Uint8Array([0, 0, 0, 255]));
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, wrapX ? gl.REPEAT : gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  }

  let source = (await decodeOffThread(url, crossOrigin)) || (await decodeWithImage(url, crossOrigin));
  if (!source) {
    console.error(`texture failed: ${url}`);
    return false;
  }

  // An 8K texture is fine on any desktop GPU, but not on every one; scale
  // down to what this one accepts rather than failing outright.
  const max = gl.getParameter(gl.MAX_TEXTURE_SIZE);
  if (source.width > max || source.height > max) {
    const k = max / Math.max(source.width, source.height);
    const c = document.createElement("canvas");
    c.width = Math.floor(source.width * k);
    c.height = Math.floor(source.height * k);
    c.getContext("2d").drawImage(source, 0, 0, c.width, c.height);
    source = c;
  }

  gl.activeTexture(gl.TEXTURE0 + unit);
  gl.bindTexture(gl.TEXTURE_2D, texture);
  gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source);
  sizes.set(unit, [source.width, source.height]);
  gl.generateMipmap(gl.TEXTURE_2D);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
  const aniso = gl.getExtension("EXT_texture_filter_anisotropic");
  if (aniso) {
    const cap = gl.getParameter(aniso.MAX_TEXTURE_MAX_ANISOTROPY_EXT);
    gl.texParameterf(gl.TEXTURE_2D, aniso.TEXTURE_MAX_ANISOTROPY_EXT, Math.min(8, cap));
  }
  if (typeof source.close === "function") source.close();
  return true;
}

// --- small vector helpers --------------------------------------------------

/// The texture object currently loaded into `unit` by loadTexture, so a
/// post-processing pass can read it.
export function textureOf(unit) {
  return textures.get(unit) || null;
}

/// [width, height] of what loadTexture last uploaded into `unit`.
export function textureSizeOf(unit) {
  return sizes.get(unit) || null;
}

export function cross(a, b) {
  return [
    a[1] * b[2] - a[2] * b[1],
    a[2] * b[0] - a[0] * b[2],
    a[0] * b[1] - a[1] * b[0],
  ];
}

export function dot(a, b) {
  return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
}

export function normalize(v) {
  const len = Math.hypot(v[0], v[1], v[2]) || 1;
  return [v[0] / len, v[1] / len, v[2] / len];
}

export function scale(v, k) {
  return [v[0] * k, v[1] * k, v[2] * k];
}

export function sub(a, b) {
  return [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
}
