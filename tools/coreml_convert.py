"""Convert LightCraft's inpainting models (LaMa, MI-GAN) from their pinned ONNX files to Core ML, check them against
onnxruntime and time them on this Mac's CPU, GPU and Neural Engine. A spike (docs/ai-models.md → Core ML), not part of
the build: nothing here ships and no model is committed.

    python3.12 -m venv .venv-coreml
    .venv-coreml/bin/pip install -r tools/requirements-coreml.txt
    .venv-coreml/bin/python tools/coreml_convert.py lama  --onnx lama_fp32.onnx  --photo some.jpg --work /tmp/coreml
    .venv-coreml/bin/python tools/coreml_convert.py migan --onnx migan.onnx      --photo some.jpg --work /tmp/coreml

`--onnx` is the file LightCraft downloads (crates/erase: `migan.onnx`, `lama_fp32.onnx`, SHA-256 pinned there).
`--photo` is any picture: a 512 x 512 crop of it gets an ellipse hole. Results land in `<work>/<model>_result.json` and
the Core ML package in `<work>/<model>_fp16.mlpackage`.

Two things the conversion needs (found 2026-10-15):
- Fixed 512 x 512 shapes folded by onnx-simplifier: LaMa's graph is 17,480 nodes of FFT arithmetic, 3,028 after.
- LaMa's inverse FFT rebuilds its spectrum with reverse slices (`x[1:-1][::-1]`); Core ML mis-infers a negative-step
  slice's shape ("Invalid target shape in reshape: [62, 12288] to [64, 12288]"), so they become constant `Gather`s.
MI-GAN's published file takes and answers uint8: its casts to uint8 become identities (what `vision::onnxfix` does for
rten) and its input and output are declared float, in 0..255.
"""
import argparse, json, subprocess, time

import numpy as np
import onnx
import onnxruntime as ort
import torch
import onnx2torch
import coremltools as ct
from onnx import TensorProto, helper, numpy_helper, shape_inference
from onnxsim import simplify
from PIL import Image


def log(*a):
    print(*a, flush=True)


def test_input(photo, which):
    """A 512 x 512 crop of `photo` and an ellipse hole, in each model's own convention."""
    im = Image.open(photo).convert("RGB")
    s = min(im.size)
    crop = im.crop((0, 0, s, s)).resize((512, 512), Image.LANCZOS)
    rgb = np.asarray(crop, dtype=np.float32)
    yy, xx = np.mgrid[0:512, 0:512]
    hole = ((xx - 250) / 70.0) ** 2 + ((yy - 300) / 110.0) ** 2 < 1.0
    if which == "lama":  # 0..1 planar RGB, mask 1 = fill, answers 0..255
        return (rgb / 255.0).transpose(2, 0, 1)[None].astype(np.float32), hole[None, None].astype(np.float32)
    # MI-GAN: 0..255, mask 255 = keep and 0 = fill, answers 0..255
    return rgb.transpose(2, 0, 1)[None].astype(np.float32), np.where(hole, 0.0, 255.0)[None, None].astype(np.float32)


def prepare(src, which, dst):
    """The ONNX file with fixed shapes folded and what Core ML can't take rewritten."""
    m = onnx.load(src)
    g = m.graph
    if which == "migan":
        for n in g.node:
            if n.op_type == "Cast" and any(a.name == "to" and a.i == TensorProto.UINT8 for a in n.attribute):
                n.op_type = "Identity"
                del n.attribute[:]
        for v in list(g.input) + list(g.output):
            if v.type.tensor_type.elem_type == TensorProto.UINT8:
                v.type.tensor_type.elem_type = TensorProto.FLOAT
    sm, ok = simplify(m, overwrite_input_shapes={"image": [1, 3, 512, 512], "mask": [1, 1, 512, 512]})
    log("simplified", ok, len(g.node), "->", len(sm.graph.node), "nodes")
    sm = shape_inference.infer_shapes(sm)
    gg = sm.graph
    shapes = {v.name: [d.dim_value for d in v.type.tensor_type.shape.dim] for v in list(gg.value_info) + list(gg.input) + list(gg.output)}
    init = {i.name: numpy_helper.to_array(i) for i in gg.initializer}
    nodes, fixed = [], 0
    for n in gg.node:
        ins = list(n.input) + [""] * (5 - len(n.input))
        data, starts, ends, axes, steps = ins[:5]
        if n.op_type != "Slice" or not (starts in init and ends in init and (not axes or axes in init) and (not steps or steps in init)):
            nodes.append(n)
            continue
        st = init[steps].tolist() if steps else [1] * len(init[starts])
        if all(s > 0 for s in st):
            nodes.append(n)
            continue
        shp = shapes[data]
        ax = init[axes].tolist() if axes else list(range(len(init[starts])))
        cur = data
        spec = list(zip(ax, init[starts].tolist(), init[ends].tolist(), st))
        for k, (a, b, e, s) in enumerate(spec):
            a %= len(shp)
            idx = np.arange(shp[a])[slice(b, e, s)].astype(np.int64)
            gg.initializer.append(numpy_helper.from_array(idx, f"{n.name}_idx{k}"))
            o = n.output[0] if k == len(spec) - 1 else f"{n.name}_g{k}"
            nodes.append(helper.make_node("Gather", [cur, f"{n.name}_idx{k}"], [o], axis=a, name=f"{n.name}_gather{k}"))
            cur = o
            shp = list(shp)
            shp[a] = len(idx)
        fixed += 1
    del gg.node[:]
    gg.node.extend(nodes)
    del gg.value_info[:]
    log("reverse slices rewritten as Gather:", fixed)
    onnx.save(sm, dst)
    return fixed


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("model", choices=["lama", "migan"])
    ap.add_argument("--onnx", required=True)
    ap.add_argument("--photo", required=True)
    ap.add_argument("--work", required=True)
    a = ap.parse_args()
    res = {"model": a.model}
    img, mask = test_input(a.photo, a.model)
    ready = f"{a.work}/{a.model}_ready.onnx"
    res["reverse_slices_fixed"] = prepare(a.onnx, a.model, ready)
    t = time.time()
    ref = ort.InferenceSession(ready, providers=["CPUExecutionProvider"]).run(None, {"image": img, "mask": mask})[0]
    res["onnxruntime_cpu_s"] = round(time.time() - t, 2)
    tm = onnx2torch.convert(ready).eval()
    with torch.no_grad():
        res["torch_max_abs_diff"] = float(np.abs(tm(torch.from_numpy(img), torch.from_numpy(mask)).numpy() - ref).max())
        traced = torch.jit.trace(tm, (torch.from_numpy(img), torch.from_numpy(mask)), strict=False)
    log("torch vs onnxruntime, max abs diff on the 0..255 scale:", res["torch_max_abs_diff"])
    t = time.time()
    ml = ct.convert(
        traced,
        inputs=[ct.TensorType(name="image", shape=(1, 3, 512, 512)), ct.TensorType(name="mask", shape=(1, 1, 512, 512))],
        outputs=[ct.TensorType(name="output")],
        convert_to="mlprogram",
        compute_precision=ct.precision.FLOAT16,
        minimum_deployment_target=ct.target.iOS17,
    )
    res["coreml_convert_s"] = round(time.time() - t, 1)
    pkg = f"{a.work}/{a.model}_fp16.mlpackage"
    ml.save(pkg)
    res["coreml_mb"] = round(int(subprocess.check_output(["du", "-sk", pkg]).split()[0]) / 1024, 1)
    for name, cu in (("cpu", ct.ComputeUnit.CPU_ONLY), ("cpu+gpu", ct.ComputeUnit.CPU_AND_GPU), ("cpu+ane", ct.ComputeUnit.CPU_AND_NE), ("all", ct.ComputeUnit.ALL)):
        t = time.time()
        model = ct.models.MLModel(pkg, compute_units=cu)
        load = time.time() - t
        feed = {"image": img, "mask": mask}
        out = list(model.predict(feed).values())[0]  # (the first call specializes the model for the unit)
        times = []
        for _ in range(3):
            t = time.time()
            out = list(model.predict(feed).values())[0]
            times.append(time.time() - t)
        d = np.abs(np.asarray(out) - ref)
        res[name] = {"load_s": round(load, 1), "predict_s": round(min(times), 3), "max_abs_diff": round(float(d.max()), 2), "mean_abs_diff": round(float(d.mean()), 3)}
        log(name, res[name])
    json.dump(res, open(f"{a.work}/{a.model}_result.json", "w"), indent=2)


if __name__ == "__main__":
    main()
