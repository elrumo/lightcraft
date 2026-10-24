# How well can the colour matrix of a camera nobody measured be predicted from the cameras somebody did? (Method behind
# `lightcraft_raw::spectral::generic`, `docs/camera-preview-colour.md` → "Cameras nobody measured".)
#
#   git clone https://github.com/AcademySoftwareFoundation/rawtoaces-data ../rawtoaces-data
#   git -C ../rawtoaces-data checkout v1.1.0
#   .venv-spectral/bin/python tools/spectral_generic_loo.py ../rawtoaces-data
#
# Each measured camera (rawtoaces-data, Apache-2.0) is held out in turn; predictors that never see its sensitivities
# give a white-preserving 3x3 (white-balanced camera RGB -> XYZ) under CIE D65 and A, and the error is CIEDE2000 over
# rawtoaces-data's 190 training reflectances against the camera's own best 3x3 (the floor), as `spectral_camera_table.py`
# fits it. Also with a whole maker left out (a maker nobody measured). Written from the Academy's published method; no
# rawtoaces code is used.
import json, os, sys, warnings
import numpy as np
import colour
from scipy.optimize import minimize

warnings.filterwarnings("ignore")
ROOT = sys.argv[1]
WL = np.arange(380, 781, 5)


def spectral(path, columns):
    data = json.load(open(path))["spectral_data"]["data"]["main"]
    return np.array([[data[str(w)][i] for i in range(columns)] for w in WL], dtype=float)


def illum(name):
    sd = colour.SDS_ILLUMINANTS[name]
    return np.array([sd[w] for w in WL], dtype=float)


cmf = spectral(os.path.join(ROOT, "data/cmf/cmf_1931.json"), 3)
tr = json.load(open(os.path.join(ROOT, "data/training/training_spectral.json")))["spectral_data"]["data"]["main"]
refl = np.array([tr[str(w)] for w in WL], dtype=float)
LIGHTS = {"D65": illum("D65"), "A": illum("A"), "D55": illum("D55")}

cams = {}
for f in sorted(os.listdir(os.path.join(ROOT, "data/camera"))):
    if f.endswith("_380_780_5.json"):
        p = os.path.join(ROOT, "data/camera", f)
        h = json.load(open(p))["header"]
        cams[f"{h['manufacturer']} {h['model']}"] = (h["manufacturer"], spectral(p, 3))
names = list(cams)
makers = np.array([cams[n][0] for n in names])


def scene(light):
    k = 1.0 / (cmf[:, 1] @ light)
    xyz = k * (cmf.T @ (light[:, None] * refl))  # 3 x 190, Y relative to the illuminant white
    white = k * (cmf.T @ light)
    return xyz, white


def cam_rgb(ssf, light):
    w = ssf.T @ light
    return (ssf.T @ (light[:, None] * refl)) / w[:, None], 1.0 / (w / w[1])  # wb camera rgb, wb multipliers (g=1)


def build(p, white):
    return np.stack([p[:3], p[3:], white - p[:3] - p[3:]], axis=1)


def lab(xyz, white):
    return colour.XYZ_to_Lab(xyz.T, colour.XYZ_to_xy(white))


def de(m, cam, xyz, white, metric="2000"):
    pred = lab(m @ cam, white)
    ref = lab(xyz, white)
    return colour.delta_E(ref, pred, method="CIE 2000" if metric == "2000" else "CIE 1976")


def fit_truth(cam, xyz, white):
    ref = lab(xyz, white)

    def loss(p):
        return float(np.mean(np.linalg.norm(lab(build(p, white) @ cam, white) - ref, axis=1)))

    m0 = xyz @ np.linalg.pinv(cam)
    r = minimize(loss, np.concatenate([m0[:, 0], m0[:, 1]]), method="Nelder-Mead", options={"maxiter": 6000, "xatol": 1e-7, "fatol": 1e-7})
    return build(r.x, white)


scenes = {k: scene(v) for k, v in LIGHTS.items() if k in ("D65", "A")}
truth = {}
feat = {}
for n in names:
    ssf = cams[n][1]
    truth[n] = {}
    for k in ("D65", "A"):
        xyz, white = scenes[k]
        cam, _ = cam_rgb(ssf, LIGHTS[k])
        truth[n][k] = fit_truth(cam, xyz, white)
    # features a raw file gives: Daylight (D55-ish) and Tungsten (A) preset multipliers, log r/g and b/g
    f = []
    for k in ("D55", "A"):
        _, wb = cam_rgb(ssf, LIGHTS[k])
        f += [np.log(wb[0]), np.log(wb[2])]
    feat[n] = np.array(f)

srgb = colour.RGB_COLOURSPACES["sRGB"].matrix_RGB_to_XYZ


def pvec(m):
    return np.concatenate([m[:, 0], m[:, 1]])


def predict_identity(k):
    xyz, white = scenes[k]
    d65 = scenes["D65"][1]
    cat = colour.adaptation.matrix_chromatic_adaptation_VonKries(d65, white, transform="Bradford")
    return cat @ srgb


def evaluate(h, train, predictor):
    out = {}
    for k in ("D65", "A"):
        xyz, white = scenes[k]
        cam, _ = cam_rgb(cams[h][1], LIGHTS[k])
        m = predictor(h, train, k)
        out[k] = de(m, cam, xyz, white)
    return out


def mean_pred(h, train, k):
    p = np.mean([pvec(truth[t][k]) for t in train], axis=0)
    return build(p, scenes[k][1])


def maker_pred(h, train, k):
    same = [t for t in train if cams[t][0] == cams[h][0]]
    return mean_pred(h, same if same else train, k)


def ridge_pred(feats, lam, with_maker=True):
    def pred(h, train, k):
        X = np.array([feat[t][feats] for t in train])
        mu, sd = X.mean(0), X.std(0) + 1e-9
        Xs = (X - mu) / sd
        Y = np.array([pvec(truth[t][k]) for t in train])
        ym = Y.mean(0)
        # maker offsets, shrunk
        cols = [Xs]
        if with_maker:
            mk = sorted(set(cams[t][0] for t in train))
            oh = np.array([[1.0 if cams[t][0] == m else 0.0 for m in mk] for t in train])
            cols.append(oh * 0.5)
        A = np.hstack(cols)
        W = np.linalg.solve(A.T @ A + lam * np.eye(A.shape[1]), A.T @ (Y - ym))
        xh = (feat[h][feats] - mu) / sd
        row = [xh]
        if with_maker:
            row.append(np.array([1.0 if cams[h][0] == m else 0.0 for m in mk]) * 0.5)
        return build(ym + np.concatenate(row) @ W, scenes[k][1])

    return pred


def nn_pred(feats):
    def pred(h, train, k):
        d = [np.linalg.norm(feat[t][feats] - feat[h][feats]) for t in train]
        return truth[train[int(np.argmin(d))]][k]

    return pred


def identity_pred(h, train, k):
    return predict_identity(k)


preds = {
    "camera RGB as sRGB (today's fallback)": identity_pred,
    "mean of all others": mean_pred,
    "mean of same maker": maker_pred,
    "nearest by daylight WB": nn_pred([0, 1]),
    "nearest by daylight+tungsten WB": nn_pred([0, 1, 2, 3]),
    "ridge daylight WB": ridge_pred([0, 1], 1.0, False),
    "ridge daylight WB + maker": ridge_pred([0, 1], 1.0, True),
    "ridge daylight+tungsten WB": ridge_pred([0, 1, 2, 3], 1.0, False),
    "ridge daylight+tungsten WB + maker": ridge_pred([0, 1, 2, 3], 1.0, True),
}


def report(title, holdouts, exclude):
    print(f"\n== {title}")
    print(f"{'predictor':42s} {'D65 mean':>9s} {'D65 p90':>8s} {'A mean':>8s} {'A p90':>7s}")
    rows = {k: [] for k in preds}
    floor = {"D65": [], "A": []}
    for h in holdouts:
        train = [t for t in names if t != h and not exclude(h, t)]
        for k in ("D65", "A"):
            xyz, white = scenes[k]
            cam, _ = cam_rgb(cams[h][1], LIGHTS[k])
            floor[k].append(de(truth[h][k], cam, xyz, white))
        for pn, pf in preds.items():
            rows[pn].append(evaluate(h, train, pf))
    f65 = np.concatenate(floor["D65"]); fa = np.concatenate(floor["A"])
    print(f"{'own best 3x3 (the floor)':42s} {f65.mean():9.2f} {np.percentile(f65, 90):8.2f} {fa.mean():8.2f} {np.percentile(fa, 90):7.2f}")
    for pn, r in rows.items():
        a = np.concatenate([x["D65"] for x in r]); b = np.concatenate([x["A"] for x in r])
        print(f"{pn:42s} {a.mean():9.2f} {np.percentile(a, 90):8.2f} {b.mean():8.2f} {np.percentile(b, 90):7.2f}")


# 1. plain LOO
report("leave one camera out (delta E 2000, 190 training reflectances)", names, lambda h, t: False)
# 2. leave the whole maker out: the case of a maker with no measured camera
for mk in ("Canon", "Nikon", "Sony"):
    report(f"leave the whole maker out: held-out {mk}", [n for n in names if cams[n][0] == mk], lambda h, t, mk=mk: cams[t][0] == mk)
