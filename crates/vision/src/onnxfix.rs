//! Making an ONNX file ready for inference with rten, in memory:
//!
//! - `Dropout` nodes (which only do something while training, and which older exports spell with
//!   a `ratio` attribute rten doesn't take) become `Identity`;
//! - every operator gets a name of its own (`<name>.op`): some exports (MXNet's, like SFace) name
//!   each operator after the value it produces, and rten looks values up by name, so a graph
//!   where `fc1` is both a BatchNorm and its output can't be planned.
//!
//! The file on disk is never touched, so what is checked against its SHA-256 is what the vendor
//! published. This rewrites the protobuf by field, without a protobuf library: a model is a
//! message holding a graph, a graph holds nodes, and only the nodes change; every other byte is
//! copied as it is.

use crate::Error;

/// Largest model accepted, in bytes.
const MAX_MODEL: usize = 512 << 20;

/// `ModelProto.graph`, `GraphProto.node`, and the `NodeProto` fields we touch.
const MODEL_GRAPH: u64 = 7;
const GRAPH_NODE: u64 = 1;
const NODE_INPUT: u64 = 1;
const NODE_OUTPUT: u64 = 2;
const NODE_NAME: u64 = 3;
const NODE_OP_TYPE: u64 = 4;
const NODE_ATTRIBUTE: u64 = 5;
/// Added to every operator's name (see the module docs).
const NAME_SUFFIX: &str = ".op";

/// One field of a protobuf message: its number, wire type and payload (a varint's own bytes; a
/// fixed value's bytes; the contents of a length-delimited field).
struct Field<'a> {
    number: u64,
    wire: u8,
    payload: &'a [u8],
}

fn bad(why: &str) -> Error {
    Error::Model(format!("the ONNX file is damaged: {why}"))
}

fn varint(buf: &[u8], at: &mut usize) -> Result<u64, Error> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*at).ok_or_else(|| bad("cut short"))?;
        *at += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(bad("a number is too long"))
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn fields(buf: &[u8]) -> Result<Vec<Field<'_>>, Error> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < buf.len() {
        let tag = varint(buf, &mut at)?;
        let (number, wire) = (tag >> 3, (tag & 7) as u8);
        let start = at;
        let payload = match wire {
            0 => {
                varint(buf, &mut at)?;
                buf.get(start..at)
            }
            1 => {
                at = at.checked_add(8).ok_or_else(|| bad("a field is too long"))?;
                buf.get(start..at)
            }
            5 => {
                at = at.checked_add(4).ok_or_else(|| bad("a field is too long"))?;
                buf.get(start..at)
            }
            2 => {
                let len = usize::try_from(varint(buf, &mut at)?).map_err(|_| bad("a field is too long"))?;
                let from = at;
                at = at.checked_add(len).ok_or_else(|| bad("a field is too long"))?;
                buf.get(from..at)
            }
            _ => return Err(bad("an unknown kind of field")),
        };
        out.push(Field { number, wire, payload: payload.ok_or_else(|| bad("cut short"))? });
    }
    Ok(out)
}

fn put_field(out: &mut Vec<u8>, number: u64, wire: u8, payload: &[u8]) {
    put_varint(out, number << 3 | u64::from(wire));
    if wire == 2 {
        put_varint(out, payload.len() as u64);
    }
    out.extend_from_slice(payload);
}

/// A node with its own name, and a `Dropout` as an `Identity` (its first input to its first output).
fn fix_node(node: &[u8]) -> Result<Vec<u8>, Error> {
    let fs = fields(node)?;
    let is_dropout = fs.iter().any(|f| f.number == NODE_OP_TYPE && f.payload == b"Dropout");
    let mut out = Vec::with_capacity(node.len() + 8);
    let (mut inputs, mut outputs) = (0, 0);
    for f in &fs {
        if !is_dropout {
            if f.number == NODE_NAME {
                let mut name = f.payload.to_vec();
                name.extend_from_slice(NAME_SUFFIX.as_bytes());
                put_field(&mut out, f.number, f.wire, &name);
            } else {
                put_field(&mut out, f.number, f.wire, f.payload);
            }
            continue;
        }
        match f.number {
            NODE_INPUT if inputs == 0 => {
                inputs += 1;
                put_field(&mut out, f.number, f.wire, f.payload);
            }
            NODE_OUTPUT if outputs == 0 => {
                outputs += 1;
                put_field(&mut out, f.number, f.wire, f.payload);
            }
            // the other inputs (ratio, training mode), the mask output and the attributes: gone
            NODE_INPUT | NODE_OUTPUT | NODE_ATTRIBUTE => {}
            NODE_OP_TYPE => put_field(&mut out, NODE_OP_TYPE, 2, b"Identity"),
            NODE_NAME => {
                let mut name = f.payload.to_vec();
                name.extend_from_slice(NAME_SUFFIX.as_bytes());
                put_field(&mut out, f.number, f.wire, &name);
            }
            _ => put_field(&mut out, f.number, f.wire, f.payload),
        }
    }
    Ok(out)
}

/// `model` made ready for rten (see the module docs).
pub fn inference_only(model: &[u8]) -> Result<Vec<u8>, Error> {
    if model.len() > MAX_MODEL {
        return Err(bad("it is too large"));
    }
    let mut out = Vec::with_capacity(model.len());
    for f in fields(model)? {
        if f.number != MODEL_GRAPH || f.wire != 2 {
            put_field(&mut out, f.number, f.wire, f.payload);
            continue;
        }
        let mut graph = Vec::with_capacity(f.payload.len());
        for g in fields(f.payload)? {
            if g.number == GRAPH_NODE && g.wire == 2 {
                put_field(&mut graph, GRAPH_NODE, 2, &fix_node(g.payload)?);
            } else {
                put_field(&mut graph, g.number, g.wire, g.payload);
            }
        }
        put_field(&mut out, MODEL_GRAPH, 2, &graph);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(out: &mut Vec<u8>, number: u64, s: &str) {
        put_field(out, number, 2, s.as_bytes());
    }

    /// A node: op type, name, inputs, outputs, and one attribute.
    fn node(op: &str, name: &str, ins: &[&str], outs: &[&str]) -> Vec<u8> {
        let mut n = Vec::new();
        for i in ins {
            string(&mut n, NODE_INPUT, i);
        }
        for o in outs {
            string(&mut n, NODE_OUTPUT, o);
        }
        string(&mut n, NODE_NAME, name);
        string(&mut n, NODE_OP_TYPE, op);
        // an attribute `ratio` (name = 1: "ratio", f = 2: fixed32 0.5, type = 20: 1)
        let mut a = Vec::new();
        string(&mut a, 1, "ratio");
        put_field(&mut a, 2, 5, &0.5f32.to_le_bytes());
        put_field(&mut a, 20, 0, &[1]);
        put_field(&mut n, NODE_ATTRIBUTE, 2, &a);
        n
    }

    fn model(nodes: &[Vec<u8>]) -> Vec<u8> {
        let mut g = Vec::new();
        for n in nodes {
            put_field(&mut g, GRAPH_NODE, 2, n);
        }
        string(&mut g, 2, "main");
        let mut m = Vec::new();
        put_field(&mut m, 1, 0, &[8]); // ir_version
        string(&mut m, 2, "test");
        put_field(&mut m, MODEL_GRAPH, 2, &g);
        string(&mut m, 4, "after the graph");
        m
    }

    fn ops(model: &[u8]) -> Vec<(String, String, usize, usize, usize)> {
        let graph = fields(model).unwrap().into_iter().find(|f| f.number == MODEL_GRAPH).unwrap();
        fields(graph.payload)
            .unwrap()
            .into_iter()
            .filter(|f| f.number == GRAPH_NODE)
            .map(|f| {
                let fs = fields(f.payload).unwrap();
                let get = |n: u64| fs.iter().find(|x| x.number == n).map(|x| String::from_utf8(x.payload.to_vec()).unwrap()).unwrap_or_default();
                let count = |n: u64| fs.iter().filter(|x| x.number == n).count();
                (get(NODE_OP_TYPE), get(NODE_NAME), count(NODE_INPUT), count(NODE_OUTPUT), count(NODE_ATTRIBUTE))
            })
            .collect()
    }

    #[test]
    fn dropout_becomes_identity_and_operators_get_their_own_names() {
        let m = model(&[
            node("Conv", "conv1", &["x", "w"], &["y"]),
            node("Dropout", "dropout5", &["y", "ratio"], &["z", "mask"]),
            node("Gemm", "fc", &["z"], &["out"]),
        ]);
        let fixed = inference_only(&m).unwrap();
        assert_eq!(
            ops(&fixed),
            [
                ("Conv".to_string(), "conv1.op".to_string(), 2, 1, 1),
                ("Identity".to_string(), "dropout5.op".to_string(), 1, 1, 0),
                ("Gemm".to_string(), "fc.op".to_string(), 1, 1, 1)
            ]
        );
        // the fields around the graph are kept
        assert!(fixed.ends_with(b"after the graph"));
        assert!(fixed.windows(4).any(|w| w == b"test"));
    }

    #[test]
    fn only_the_names_change_in_a_model_without_dropout() {
        let m = model(&[node("Conv", "c", &["x"], &["y"]), node("Relu", "r", &["y"], &["z"])]);
        let fixed = inference_only(&m).unwrap();
        assert_eq!(ops(&fixed).iter().map(|o| o.1.as_str()).collect::<Vec<_>>(), ["c.op", "r.op"]);
        assert_eq!(fixed.len(), m.len() + 2 * NAME_SUFFIX.len());
        // names are all that differs: a second pass is only the suffix again
        assert_eq!(ops(&inference_only(&fixed).unwrap()).iter().map(|o| o.1.as_str()).collect::<Vec<_>>(), ["c.op.op", "r.op.op"]);
        assert_eq!(inference_only(&[]).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn damaged_files_are_errors_not_panics() {
        let m = model(&[node("Dropout", "d", &["y"], &["z"])]);
        for cut in [1, 2, 5, 17, m.len() / 2, m.len() - 1] {
            // (some cuts are valid prefixes of a message; none may panic)
            let _ = inference_only(&m[..cut]);
        }
        assert!(inference_only(&[0xff; 20]).is_err(), "an endless number");
        assert!(inference_only(&[0x3a, 0xff, 0xff, 0xff, 0xff, 0x0f]).is_err(), "a length past the end");
        assert!(inference_only(&[0x0b]).is_err(), "a wire type that doesn't exist");
        let mut s = 7u32;
        for _ in 0..200 {
            let junk: Vec<u8> = (0..64)
                .map(|_| {
                    s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                    (s >> 24) as u8
                })
                .collect();
            let _ = inference_only(&junk);
        }
    }
}
