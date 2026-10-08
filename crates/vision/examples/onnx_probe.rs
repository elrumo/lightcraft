//! What an ONNX model asks for and whether rten can run it: prints its inputs and outputs, then runs
//! it once on zeros.
//!
//! cargo run --release -p lightcraft-vision --features onnx --example onnx_probe -- MODEL.onnx [H W]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rten::{Dimension, ValueView};
use rten_tensor::Layout;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("usage: onnx_probe MODEL.onnx [H W]");
    let t = std::time::Instant::now();
    let bytes = std::fs::read(path).unwrap();
    let bytes = if std::env::var_os("FIX").is_some() { lightcraft_vision::onnxfix::inference_only(&bytes).unwrap() } else { bytes };
    let mut opts = rten::ModelOptions::with_all_ops();
    if std::env::var_os("NOOPT").is_some() {
        opts.enable_optimization(false);
    }
    if std::env::var_os("PREPACK").is_some() {
        opts.prepack_weights(true);
    }
    let model = opts.load(bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
    println!("loaded in {:?}", t.elapsed());
    let describe = |id| {
        let info = model.node_info(id).unwrap();
        let shape: Vec<String> = info
            .shape()
            .map(|s| {
                s.iter()
                    .map(|d| match d {
                        Dimension::Fixed(n) => n.to_string(),
                        Dimension::Symbolic(n) => n.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        format!("{:?} {:?}", info.name().unwrap_or("?"), shape)
    };
    for id in model.input_ids().iter().take(4) {
        println!("input  {}", describe(*id));
    }
    println!("({} graph inputs in all)", model.input_ids().len());
    for id in model.output_ids() {
        println!("output {}", describe(*id));
    }
    // run once on zeros: the input's fixed dimensions, the given height and width for the dynamic ones
    let (h, w): (usize, usize) = (args.get(1).and_then(|v| v.parse().ok()).unwrap_or(640), args.get(2).and_then(|v| v.parse().ok()).unwrap_or(640));
    let input = model.input_ids()[0];
    let info = model.node_info(input).unwrap();
    let dims: Vec<usize> = info
        .shape()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, d)| match d {
            Dimension::Fixed(n) => *n,
            Dimension::Symbolic(_) => [1, 3, h, w][i.min(3)],
        })
        .collect();
    let data = vec![0.0f32; dims.iter().product()];
    let value = ValueView::from_shape(dims.as_slice(), data.as_slice()).unwrap();
    // OUT=a,b,c: stop at these values instead of the model's outputs (finding where a model breaks)
    let outputs: Vec<_> = match std::env::var("OUT") {
        Ok(names) => names.split(',').map(|n| model.node_id(n).unwrap_or_else(|e| panic!("{n}: {e}"))).collect(),
        Err(_) => model.output_ids().to_vec(),
    };
    let t = std::time::Instant::now();
    match model.run(vec![(input, value.into())], &outputs, None) {
        Ok(out) => {
            println!("ran on {dims:?} in {:?}", t.elapsed());
            for (id, o) in outputs.iter().zip(out) {
                println!("  {:?} -> {:?}", model.node_info(*id).unwrap().name(), o.shape());
            }
        }
        Err(e) => println!("RUN FAILED: {e}"),
    }
}
