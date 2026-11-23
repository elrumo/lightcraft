//! The face models a native frontend runs itself (card #516): YuNet and SFace converted for its machine-learning runtime,
//! run in place of the interpreter's CPU kernels through the nets' accelerator seam ([`lightcraft_faces::net::Accelerator`]),
//! so the check that it agrees with the CPU and the fall back to the CPU on its first error stay the interpreter's.
//!
//! On an M1 Max a photo's detection took 9 ms and a face's embedding 4 ms there; over 44 photos the same 109 of 110 faces
//! were found (the one more sat on the score line), boxes within 1.7 pixels of the 640 square, and the embeddings at a
//! cosine of 0.99998 to the CPU's.

use std::path::PathBuf;
use std::sync::Arc;

use lightcraft_faces::net::{Accelerator, ConvJob, program::Program};
use lightcraft_vision::host::{self, Input, Request, Units};

/// A face model's twin: the model it was converted from (by its file's SHA-256), its id among the downloads, and its input's name and shape.
struct Twin {
    sha256: &'static str,
    id: &'static str,
    input: &'static str,
    shape: [usize; 4],
}

/// The models with a twin (their ids are `lightcraft_enhance::models`' YUNET_COREML and SFACE_COREML).
const TWINS: [Twin; 2] = [
    Twin { sha256: lightcraft_faces::known::YUNET_SHA256, id: YUNET_ID, input: "input", shape: [1, 3, 640, 640] },
    Twin { sha256: lightcraft_faces::known::SFACE_SHA256, id: SFACE_ID, input: "data", shape: [1, 3, 112, 112] },
];

const YUNET_ID: &str = lightcraft_vision::faces::HOST_DETECTOR;
const SFACE_ID: &str = lightcraft_vision::faces::HOST_RECOGNISER;

/// The frontend's twin of the face model whose file has `sha256`, when a frontend runs models and the twin is downloaded and on.
pub(crate) fn twin(s: &crate::Session, sha256: Option<&str>) -> Option<Arc<dyn Accelerator>> {
    let t = TWINS.iter().find(|t| Some(t.sha256) == sha256)?;
    let dir = s.enhancer.host_dir(t.id)?;
    Some(Arc::new(HostNet { id: t.id, dir, input: t.input, shape: t.shape }))
}

struct HostNet {
    id: &'static str,
    dir: PathBuf,
    input: &'static str,
    shape: [usize; 4],
}

impl Accelerator for HostNet {
    fn device(&self) -> String {
        "the system's machine-learning runtime".into()
    }

    fn half_precision(&self) -> bool {
        true
    }

    fn conv(&self, _job: &ConvJob) -> Result<Option<Vec<f32>>, String> {
        Ok(None)
    }

    /// Each input through the twin, whose one answer is the net's outputs one after another, in the program's order.
    fn run_program(&self, program: &Program, inputs: &[&[f32]]) -> Result<Option<Vec<Vec<Vec<f32>>>>, String> {
        let lens: Vec<usize> = program.outputs.iter().map(|(_, _, shape)| shape.iter().product()).collect();
        let total = lens.iter().sum();
        let mut all = Vec::with_capacity(inputs.len());
        for data in inputs {
            // (another size than the twin's: the CPU's)
            if data.len() != self.shape.iter().product::<usize>() {
                return Ok(None);
            }
            let flat = host::run(&Request {
                model: self.id,
                dir: &self.dir,
                function: None,
                units: Units::All,
                inputs: &[Input { name: self.input, shape: self.shape.to_vec(), data }],
                output: "output",
                output_len: total,
            })?;
            let mut outs = Vec::with_capacity(lens.len());
            let mut at = 0;
            for n in &lens {
                outs.push(flat.get(at..at + n).ok_or("the model answered too few numbers")?.to_vec());
                at += n;
            }
            all.push(outs);
        }
        Ok(Some(all))
    }
}
