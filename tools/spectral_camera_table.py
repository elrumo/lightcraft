# Writes crates/raw/src/spectral_table.rs: DNG-shaped colour matrices per camera model, fitted to the
# measured spectral sensitivities in rawtoaces-data (Academy Software Foundation, Apache-2.0).
#
#   git clone https://github.com/AcademySoftwareFoundation/rawtoaces-data ../rawtoaces-data
#   git -C ../rawtoaces-data checkout v1.1.0
#   python3 -m venv .venv-spectral && .venv-spectral/bin/pip install -r tools/requirements-spectral-camera-table.txt
#   .venv-spectral/bin/python tools/spectral_camera_table.py ../rawtoaces-data crates/raw/src/spectral_table.rs
#
# Method (Academy P-2013-001, written from its description; no rawtoaces code is used): the camera and the CIE 1931
# observer both look at the 190 training reflectances of rawtoaces-data under CIE Standard Illuminant A and under D65.
# The camera values are white balanced to that illuminant, and a white-preserving 3x3 to XYZ is fitted by least
# mean CIE76 error in Lab. Per illuminant this gives ColorMatrix (XYZ -> camera, DNG CalibrationIlluminant 17 / 21)
# and ForwardMatrix (white-balanced camera -> XYZ D50). The aliases are rawtoaces-data's own `data/aliases.json`.
#
# GENERIC is the colour model of a camera nobody measured: the mean of the measured cameras' matrices, per maker for the
# makers with at least GENERIC_MIN_MAKER of them (Canon, Nikon, Sony), else over all of them. Leaving each camera out in turn and predicting
# its matrices from the others, the mean error on the 190 training reflectances (CIEDE2000, D65) is 2.0 (the camera's
# own best matrix: 1.15; camera RGB read as linear sRGB, which is what a camera without matrices gets otherwise: 8.7);
# leaving a whole maker out it is 1.7 to 2.7, so a maker with no measured camera at all is covered too
# (tools/spectral_generic_loo.py reproduces those numbers).
import json
import os
import subprocess
import sys

import colour
import numpy as np
from scipy.optimize import minimize

WL = np.arange(380, 781, 5)
D50 = colour.xy_to_XYZ(colour.CCS_ILLUMINANTS["CIE 1931 2 Degree Standard Observer"]["D50"])


def spectral(path, columns):
    data = json.load(open(path))["spectral_data"]["data"]["main"]
    return np.array([[data[str(w)][i] for i in range(columns)] for w in WL], dtype=float)


def illuminant(name):
    sd = colour.SDS_ILLUMINANTS[name]
    return np.array([sd[w] for w in WL], dtype=float)


def fit(cmf, refl, ssf, light, target_white):
    """White-balanced camera RGB -> XYZ adapted to `target_white`; maps (1, 1, 1) to `target_white`."""
    w = ssf.T @ light
    cam = (ssf.T @ (light[:, None] * refl)) / w[:, None]
    k = 1.0 / (cmf[:, 1] @ light)
    xyz = k * (cmf.T @ (light[:, None] * refl))
    white = k * (cmf.T @ light)
    cat = colour.adaptation.matrix_chromatic_adaptation_VonKries(white, target_white, transform="Bradford")
    xyz = cat @ xyz
    target_xy = colour.XYZ_to_xy(target_white)
    lab = colour.XYZ_to_Lab(xyz.T, target_xy)

    def build(p):
        return np.stack([p[:3], p[3:], target_white - p[:3] - p[3:]], axis=1)

    def loss(p):
        return float(np.mean(np.linalg.norm(colour.XYZ_to_Lab((build(p) @ cam).T, target_xy) - lab, axis=1)))

    m0 = xyz @ np.linalg.pinv(cam)
    r = minimize(loss, np.concatenate([m0[:, 0], m0[:, 1]]), method="Nelder-Mead", options={"maxiter": 20000, "xatol": 1e-9, "fatol": 1e-9})
    return build(r.x), loss(r.x)


def dng_tags(cmf, refl, ssf):
    """[(ColorMatrix, ForwardMatrix, mean training dE76)] for Standard Illuminant A, then D65."""
    out = []
    for name in ("A", "D65"):
        light = illuminant(name)
        forward, error = fit(cmf, refl, ssf, light, D50)
        k = 1.0 / (cmf[:, 1] @ light)
        white = k * (cmf.T @ light)
        to_xyz, _ = fit(cmf, refl, ssf, light, white)
        cam_from_xyz = np.diag(ssf.T @ light) @ np.linalg.inv(to_xyz)
        cam_from_xyz = cam_from_xyz / (cam_from_xyz @ white).max()
        out.append((cam_from_xyz.round(6), forward.round(6), error))
    return out


GENERIC_MIN_MAKER = 5
# Aliases of table cameras that rawtoaces-data's list lacks: (make, name as sold, table model, comment).
EXTRA_ALIASES = [("Canon", "EOS R6 Mark II", "EOS R6m2", "same model, alternative name")]


def mat(m):
    return "Mat3([" + ", ".join("[" + ", ".join(f"{v:.6}" for v in row) + "]" for row in m) + "])"


def main(root, out):
    commit = subprocess.run(["git", "-C", root, "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    cmf = spectral(os.path.join(root, "data/cmf/cmf_1931.json"), 3)
    training = json.load(open(os.path.join(root, "data/training/training_spectral.json")))["spectral_data"]["data"]["main"]
    refl = np.array([training[str(w)] for w in WL], dtype=float)
    rows = []
    tags = []  # (maker, [ColorMatrix1, ColorMatrix2], [ForwardMatrix1, ForwardMatrix2])
    for f in sorted(os.listdir(os.path.join(root, "data/camera"))):
        if not f.endswith("_380_780_5.json"):
            continue
        path = os.path.join(root, "data/camera", f)
        header = json.load(open(path))["header"]
        if header.get("license") != "Apache-2.0":
            sys.exit(f"{f}: licence {header.get('license')!r}, expected Apache-2.0")
        (cm1, fm1, e1), (cm2, fm2, e2) = dng_tags(cmf, refl, spectral(path, 3))
        print(f"{f}: training dE76 A {e1:.3f}, D65 {e2:.3f}", file=sys.stderr)
        tags.append((header["manufacturer"], [cm1, cm2], [fm1, fm2]))
        rows.append(
            f'    Camera {{\n        make: "{header["manufacturer"]}",\n        model: "{header["model"]}",\n'
            f"        color_matrix: [{mat(cm1)}, {mat(cm2)}],\n        forward_matrix: [{mat(fm1)}, {mat(fm2)}],\n    }},"
        )
    aliases = json.load(open(os.path.join(root, "data/aliases.json")))["data"]["camera"]["model"]
    alias_rows = [
        f'    ("{make}", "{alias}", "{target["model"]}"), // {target.get("comments", "")}'
        for make, models in aliases.items()
        for alias, target in models.items()
    ] + [f'    ("{make}", "{alias}", "{model}"), // {comment}' for make, alias, model, comment in EXTRA_ALIASES]
    makers = sorted({t[0] for t in tags})
    groups = [("", tags)] + [(m, [t for t in tags if t[0] == m]) for m in makers if sum(t[0] == m for t in tags) >= GENERIC_MIN_MAKER]
    generic_rows = []
    for maker, group in groups:
        cms = [np.mean([t[1][i] for t in group], axis=0).round(6) for i in range(2)]
        fms = [np.mean([t[2][i] for t in group], axis=0).round(6) for i in range(2)]
        label = f"typical {maker}" if maker else "typical camera"
        generic_rows.append(
            f'Camera {{\n        make: "{maker}",\n        model: "{label} ({len(group)} measured)",\n'
            f"        color_matrix: [{mat(cms[0])}, {mat(cms[1])}],\n        forward_matrix: [{mat(fms[0])}, {mat(fms[1])}],\n    }}"
        )
    rust = (
        "// Generated by tools/spectral_camera_table.py from rawtoaces-data "
        f"(https://github.com/AcademySoftwareFoundation/rawtoaces-data, commit {commit}).\n"
        "// Derived from the camera spectral sensitivities of rawtoaces-data (Academy Software Foundation), licensed under\n"
        "// the Apache License 2.0 (LICENSE-APACHE); see NOTICE and assets/ATTRIBUTION.md. Do not edit by hand.\n"
        "use super::Camera;\n"
        "use lightcraft_color::Mat3;\n\n"
        "#[rustfmt::skip]\npub(super) const CAMERAS: &[Camera] = &[\n" + "\n".join(rows) + "\n];\n\n"
        "/// The mean of all the measured cameras' matrices: the colour model of a camera nobody measured. See\n"
        "/// `tools/spectral_camera_table.py`.\n"
        "#[rustfmt::skip]\npub(super) const GENERIC: Camera = " + generic_rows[0] + ";\n\n"
        "/// The same per maker with at least five measured cameras (Canon, Nikon, Sony).\n"
        "#[rustfmt::skip]\npub(super) const GENERIC_MAKERS: &[Camera] = &[\n    " + ",\n    ".join(generic_rows[1:]) + ",\n];\n\n"
        "/// `(make, model as sold, table model)`: other names of a camera in [`CAMERAS`], or of a camera with the same sensor.\n"
        "#[rustfmt::skip]\npub(super) const ALIASES: &[(&str, &str, &str)] = &[\n" + "\n".join(alias_rows) + "\n];\n"
    )
    open(out, "w").write(rust)
    print(f"{len(rows)} cameras, {len(alias_rows)} aliases -> {out}", file=sys.stderr)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: spectral_camera_table.py <rawtoaces-data checkout> <output .rs>")
    main(sys.argv[1], sys.argv[2])
