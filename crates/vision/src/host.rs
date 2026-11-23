//! Models a native frontend runs itself: the engine prepares a model's inputs and reads its answer as it does for its
//! own runtimes, and the frontend runs the network on the system's own machine-learning runtime, where it can use the
//! GPU and a neural accelerator (on an M1 Max, Depth Anything 3 Small took 0.47 s and 10.8 J of CPU on rten, and
//! 0.10 s and 0.03 J of CPU there). The models are downloaded like the others (pinned files in the models folder), in
//! the frontend's format; this crate only carries the call.

use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

/// Where the frontend may run a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Units {
    /// Anywhere: the neural accelerator first.
    All,
    /// The CPU and the GPU only (a model whose answers are wrong at the accelerator's precision).
    CpuAndGpu,
}

impl Units {
    pub fn key(self) -> &'static str {
        match self {
            Units::All => "all",
            Units::CpuAndGpu => "cpuAndGPU",
        }
    }
}

/// One input: its name in the model, its shape and its numbers (row-major, as many as the shape holds).
pub struct Input<'a> {
    pub name: &'a str,
    pub shape: Vec<usize>,
    pub data: &'a [f32],
}

/// One run: the model's id and its folder of downloaded files, the function of it to run (a model made of several, one
/// per input size an accelerator wants fixed; `None`: its only one), where it may run, its inputs, and its answer's name
/// and length (the frontend writes that many numbers, row-major).
pub struct Request<'a> {
    pub model: &'a str,
    pub dir: &'a Path,
    pub function: Option<&'a str>,
    pub units: Units,
    pub inputs: &'a [Input<'a>],
    pub output: &'a str,
    pub output_len: usize,
}

/// What runs a request: the answer's numbers, or why it couldn't.
pub type Runner = Arc<dyn Fn(&Request<'_>) -> Result<Vec<f32>, String> + Send + Sync>;

static RUNNER: RwLock<Option<Runner>> = RwLock::new(None);

/// Have `runner` run the models a frontend runs itself from now on (`None`: none; the engine's own runtimes run theirs).
pub fn set_runner(runner: Option<Runner>) {
    *RUNNER.write().unwrap_or_else(PoisonError::into_inner) = runner;
}

/// Whether a frontend runs models now.
pub fn available() -> bool {
    RUNNER.read().unwrap_or_else(PoisonError::into_inner).is_some()
}

/// Run `request` on the frontend: its answer, checked to be `output_len` finite numbers.
pub fn run(request: &Request<'_>) -> Result<Vec<f32>, String> {
    let runner = RUNNER.read().unwrap_or_else(PoisonError::into_inner).clone().ok_or("no frontend runs this model here")?;
    for i in request.inputs {
        if i.shape.iter().try_fold(1usize, |n, d| n.checked_mul(*d)) != Some(i.data.len()) {
            return Err(format!("input `{}` holds {} numbers, not its shape's", i.name, i.data.len()));
        }
    }
    let out = runner(request)?;
    if out.len() != request.output_len {
        return Err(format!("{} answered {} numbers, not {}", request.model, out.len(), request.output_len));
    }
    if out.iter().any(|v| !v.is_finite()) {
        return Err(format!("{} answered numbers that aren't finite", request.model));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // (one test: the runner is process-wide)
    #[test]
    fn a_frontends_runner_is_asked_and_its_answer_checked() {
        let dir = Path::new("/models/x");
        let data = [1.0f32, 2.0, 3.0, 4.0];
        let req =
            |inputs: &[Input<'_>], len| run(&Request { model: "x", dir, function: None, units: Units::All, inputs, output: "out", output_len: len });
        let input = || [Input { name: "a", shape: vec![1, 4], data: &data }];
        set_runner(None);
        assert!(!available() && req(&input(), 4).is_err(), "nobody runs it");
        set_runner(Some(Arc::new(|r: &Request<'_>| {
            let a = r.inputs.first().ok_or("no input")?;
            Ok(a.data.iter().map(|v| v * 2.0).collect())
        })));
        assert!(available());
        assert_eq!(req(&input(), 4), Ok(vec![2.0, 4.0, 6.0, 8.0]));
        assert!(req(&input(), 5).is_err(), "a short answer");
        assert!(req(&[Input { name: "a", shape: vec![2, 4], data: &data }], 4).is_err(), "an input that isn't its shape");
        set_runner(None);
        assert!(!available());
    }
}
