# Core Image against the engine's Light controls

> **Last updated:** 2026-10-27 · **Status:** measured, nothing ships from it · **Tool:** [`tools/apple_light_parity.sh`](../tools/apple_light_parity.sh)

Question: if a native (non-Rust) frontend drew the Light section (Exposure, Contrast, Highlights, Shadows, Whites,
Blacks) with Core Image instead of the engine, how close would it get? It follows [`apple-raw9.md`](apple-raw9.md),
which measured Apple as a decoder; this measures Apple as the editor.

## Method

`tools/apple_light_parity.sh --cli lightcraft-cli [--linear] [--raw FILE…] PICTURES…` renders each control at several
values with `lightcraft-cli render --set light.<control>=<value>` and with Core Image, three ways:

| Tier | What it is |
|---|---|
| `none` | The picture unedited. Its error is the size of the edit, to read the others against. |
| `builtin` | One stock filter per control (`CIExposureAdjust`, `CIColorControls`, `CIHighlightShadowAdjust`, `CIToneCurve` for whites and blacks), its strength **fitted per value** to the engine's render: the best case for that filter. |
| `curve` | The engine's own response to the control on a grey ramp, loaded as a 1-D curve (`CIColorCurves`): the ceiling for any filter that treats every pixel alike. |

Two arms. **Display-referred pictures** (4: two CC0 JPEGs and two engine neutral renders of raws): both sides start from
the same pixels, so the error is per-pixel CIEDE2000 against the engine. **Raws** (2: Canon EOS R6 CR3, Fujifilm X100
RAF): the sides start from different decodes (engine look, `CIRAWFilter`), so only the response is compared, as the
mean L\* change in the shadows (L\* < 30), mids and highlights of each side's own picture. Everything at 1600 px long
edge. `--linear` makes Core Image's working space linear light, its default; without it filters run on encoded sRGB.

## Results

Mean CIEDE2000 against the engine, median over the 4 pictures, Core Image in **linear light** (its default). Lower is closer.

| Control | Value | `none` | `builtin` | `curve` |
|---|---|---|---|---|
| Exposure | -1 / +1 EV | 11.8 / 12.5 | **0.45 / 0.69** | 0.21 / 0.58 |
| Contrast | -50 / +50 / +100 | 2.0 / 2.0 / 3.8 | 2.6 / 2.5 / 4.5 | 1.5 / 1.5 / 2.6 |
| Highlights | -50 / -100 / **+50** | 5.1 / 10.1 / 5.4 | 2.1 / 5.9 / **5.4** | 1.9 / 3.6 / 2.6 |
| Shadows | -50 / +50 / +100 | 0.7 / 0.7 / 1.3 | 0.8 / 1.1 / 2.3 | 1.0 / 1.1 / 2.0 |
| Whites | -50 / +50 | 0.7 / 0.6 | 0.6 / 0.6 | 0.5 / 0.5 |
| Blacks | -50 / +50 | 0.5 / 0.5 | 0.6 / 0.7 | 0.5 / 0.5 |

With filters on encoded sRGB instead, Contrast `builtin` drops to 1.6 / 1.6 / 2.7 (better than `none`) and Exposure
needs a strength of about 0.5 (the filter is a gain, and a gain on encoded values is not an EV): **the working space
has to be chosen per control.**

Raw arm, L\* change in shadows / mids / highlights (engine vs Core Image with the best strength), R6:

| Control | Engine | Core Image |
|---|---|---|
| Highlights -100 | 0.0 / -0.8 / -6.4 | 0.0 / -1.6 / -3.6 |
| Highlights +50 | 0.0 / 0.6 / 3.6 | 0.0 / 0.0 / 0.0 |
| Shadows +100 | 13.4 / 4.0 / 0.1 | 9.9 / 8.1 / 3.7 |
| Whites ±50 | 0.0 / ∓0.3 / ∓1.9 | 0.0 / ∓0.4..0.9 / ∓1.2..2.0 |
| Blacks ±50 | ∓0.9 / ∓0.1 / 0.0 | ∓0.6..1.1 / ∓0.3 / 0.0 |

The X100 shows the same pattern (Shadows +100: engine 21.9 / 9.7 / 1.4, Core Image 20.3 / 18.0 / 5.9).

Speed, warm render of 1920 x 1440 to a CPU bitmap (includes the read-back; strength changed on every pass), one Mac:
passthrough 0.5 ms; exposure + contrast + whites + blacks 1.8 ms; plus highlights + shadows (radius 10) 7.8 ms; a 1-D
curve 1.4 ms. Against the 16 ms draft budget, but for six controls of about 140 and with no masks.

## Reading

- **Exposure: matches** once the filter runs in linear light (error 0.5 to 0.7 against an edit of 12).
- **Whites and Blacks: match** to about 1 L\* in every zone, with tiny strengths (the stock curve has no unit that
  lines up with the slider, so a port needs its own mapping).
- **Contrast: a stock `CIColorControls` is worse than leaving the picture alone in linear light**, since it pivots at 0.5 of
  the working space. It needs the encoded space (or a log pivot around 0.18, as the engine does). Even the engine's
  response copied onto a per-channel curve closes only about a quarter of the error: the engine applies contrast to
  luminance and keeps colour ratios, which per-channel filters don't.
- **Highlights: does not match.** `CIHighlightShadowAdjust` can only darken (the +50 row is exactly `none`), reaches
  about half of the engine's -100 on the R6, and is not edge-aware the way the engine's guided-filter base layer is.
- **Shadows: close in the shadows, wrong elsewhere.** It lifts the mids about twice as much as the engine (8.1 against
  4.0; 18.0 against 9.7 on the X100) and reaches into highlights.
- The engine's Highlights and Shadows are local operators; a 1-D curve (`curve`) cannot stand in for them (error 2 to 4).

## Limits of this measurement

- Small: 4 pictures, 2 raws, 1 Mac, no iPhone. Two of the 4 pictures are the engine's own renders of raws.
- Strengths are fitted per value, not one law per slider; many sit at the end of the candidate grid
  (`... at the end of the candidates` in the TSV), so the true optimum may lie beyond it.
- The raw arm compares zone responses of different bases, not pixels; Exposure on a raw (miss 9 to 14) mixes in the
  engine's camera look against Apple's tone curve.
- Speed is a warm GPU render of this chain only; no Rust-side number is given because `lightcraft-cli` time includes
  process start. Peak memory of the run (640 to 770 MB) is dominated by decoding 20 MP pictures, so it says nothing
  about the filters.
- Not measured: Texture, Clarity, Dehaze, the mixer, masks, curves, grain, lens, healing, process versions V1 to V5.

## Decision input

Of the six, four (Exposure, Whites, Blacks, Contrast with its own pivot) are within reach of stock filters. Highlights
and Shadows are not, and they are the controls people reach for most; matching them means writing a local-contrast
kernel (a guided-filter base layer in log luminance, as `crates/pipeline/src/local.rs` does), which is re-implementing the
engine rather than calling Apple. The server and the other frontends render from the same settings, so any difference
would show between devices.
