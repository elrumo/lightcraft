// The HDR histogram of the HDR picture `finish` wrote (`Histogram::of_hdr`: `HdrBins::bin` of the linear
// values, SDR in bins 0..HIST_FROM, HIST_STOPS stops above SDR white after), counted with atomics:
// 256 bins each of R, G, B, then luma. Bindings: src (`finish`'s `out`: the 8-bit image, then the
// picture's half floats from word N on), hist (1024 counts, cleared).
// P: W (the picture's width), X0, Y0, KW, KH (the part counted: a window spots grew is cut back), N.

var<workgroup> wg_bins: array<atomic<u32>, 1024>;

fn srgb_dec(e: f32) -> f32 {
    if (e <= 0.04045) {
        return e / 12.92;
    }
    return pow((e + 0.055) / 1.055, 2.4);
}

// `HdrBins::bin` of linear `v` whose sRGB encoding is `e`.
fn hdr_bin(v: f32, e: f32) -> u32 {
    if (!(v > 0.0)) {
        return 0u;
    }
    if (v <= 1.0) {
        return min(u32(e * f32(HIST_FROM - 1u) + 0.5), HIST_FROM - 1u);
    }
    let t = clamp(log2(v) / HIST_STOPS, 0.0, 1.0);
    return min(HIST_FROM + u32(t * f32(255u - HIST_FROM) + 0.5), 255u);
}

@compute @workgroup_size(16, 16)
fn hdr_hist(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    for (var k = li; k < 1024u; k += 256u) {
        atomicStore(&wg_bins[k], 0u);
    }
    workgroupBarrier();
    if (gid.x < pu(3u) && gid.y < pu(4u)) {
        let i = (pu(2u) + gid.y) * pu(0u) + pu(1u) + gid.x;
        let o = pu(5u) + 2u * i;
        let rg = unpack2x16float(src[o]);
        let ba = unpack2x16float(src[o + 1u]);
        let e = vec3<f32>(rg.x, rg.y, ba.x);
        let c = vec3<f32>(srgb_dec(e.x), srgb_dec(e.y), srgb_dec(e.z));
        atomicAdd(&wg_bins[hdr_bin(c.x, e.x)], 1u);
        atomicAdd(&wg_bins[256u + hdr_bin(c.y, e.y)], 1u);
        atomicAdd(&wg_bins[512u + hdr_bin(c.z, e.z)], 1u);
        let l = 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        atomicAdd(&wg_bins[768u + hdr_bin(l, srgb_exact(max(l, 0.0)))], 1u);
    }
    workgroupBarrier();
    for (var k = li; k < 1024u; k += 256u) {
        let n = atomicLoad(&wg_bins[k]);
        if (n != 0u) {
            atomicAdd(&hist[k], n);
        }
    }
}

// The 8-bit picture's histogram as `Histogram::of_srgb8` counts it (R, G, B, then luma, 256 bins each, of every
// STEP-th pixel of every STEP-th row of the part kept), and in bin 1024 its pixels whose alpha is not 255 (work
// that did not run: `finish` writes 255 everywhere) over the whole picture, so no CPU pass reads the picture.
// Bindings: src (`finish`'s `out`: packed RGBA8), hist (1025 counts, cleared). P: W, H, X0, Y0, KW, KH (the part
// counted), STEP. Each invocation reads HIST8_ROWS rows of a column.
const HIST8_ROWS: u32 = 4u;

var<workgroup> wg8: array<atomic<u32>, 1025>;

@compute @workgroup_size(16, 16)
fn hist8(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    for (var k = li; k < 1025u; k += 256u) {
        atomicStore(&wg8[k], 0u);
    }
    workgroupBarrier();
    let x = gid.x;
    if (x < pu(0u)) {
        for (var r = 0u; r < HIST8_ROWS; r++) {
            let y = gid.y * HIST8_ROWS + r;
            if (y >= pu(1u)) {
                break;
            }
            let p = src[y * pu(0u) + x];
            if ((p >> 24u) != 255u) {
                atomicAdd(&wg8[1024u], 1u);
            }
            // (left of or above the part kept, the differences wrap round and fail the bounds)
            let kx = x - pu(2u);
            let ky = y - pu(3u);
            if (kx < pu(4u) && ky < pu(5u) && kx % pu(6u) == 0u && ky % pu(6u) == 0u) {
                let cr = p & 255u;
                let cg = (p >> 8u) & 255u;
                let cb = (p >> 16u) & 255u;
                atomicAdd(&wg8[cr], 1u);
                atomicAdd(&wg8[256u + cg], 1u);
                atomicAdd(&wg8[512u + cb], 1u);
                atomicAdd(&wg8[768u + min((cr * 54u + cg * 183u + cb * 19u) >> 8u, 255u)], 1u);
            }
        }
    }
    workgroupBarrier();
    for (var k = li; k < 1025u; k += 256u) {
        let n = atomicLoad(&wg8[k]);
        if (n != 0u) {
            atomicAdd(&hist[k], n);
        }
    }
}
