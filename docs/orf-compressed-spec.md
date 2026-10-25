# Compressed ORF (Olympus / OM System): format notes (PARTIAL, not yet implementable)

Last updated: 2026-10-24. Status: **incomplete.** The container, header and row-seed layout are established. The
per-pixel code (prediction, bit layout, `k` rule) is **not** established. A decoder cannot be written from this
document alone yet; section 7 says what is missing and how to close it.

## 0. Provenance and clean-room statement

- Sources: the two patents in `docs/raw-parity.md` (US7333035B2, US7715639B2), read through Google Patents, and
  black-box analysis (exiftool, Python bit-level scripts) of the sample files listed in section 6. Scratch scripts live
  in the session scratchpad (`a2.py`..`b6.py`), not in the repo.
- No dcraw, LibRaw, rawspeed, rawloader, darktable or RawTherapee source was read or opened. **Disclosure:** the
  author (an AI model) has general background knowledge that such decoders exist and roughly what families of schemes
  they use. Every hypothesis below was nevertheless generated from the patents or the bytes and tested against the
  files; nothing was ported from memory. Where a test is weak the text says so. The implementer should still treat
  section 4 candidates as hypotheses, not as a recollected design.
- No decoder code was written for the repo.

## 1. Container (verified)

- Files are TIFF-like (`IIRO` byte order mark, little-endian), IFD0 carries `Compression = 1` ("Uncompressed")
  for **both** packed-12-bit uncompressed ORF and compressed ORF. The tag does not tell them apart.
- IFD0 `StripOffsets` / `StripByteCounts` hold one strip with the whole raw stream; `ImageWidth/Height` are the full
  sensor size (E-M1 4640x3472, E-M10 IV 5200x3904, OM-1 5220x3912).
- Compressed vs uncompressed is decided by the strip: the compressed bodies seen start with the 7-byte magic
  `00 00 00 00 01 00 00` (section 2). Old bodies (E-1, E-300, C5050Z: strip begins with arbitrary bytes, and
  byte count ~ W*H*12/8) are the packed uncompressed layout. 50 of the ORF files in the survey match
  the magic (E-M1 and later, E-PL/E-P, E-3/E-5/E-4xx/E-5xx/E-6xx/E-30, XZ-1, TG-4..7, OM-1/3/5).
  Survey: `corpus2/raw` listing, first 14 bytes of each strip. Verified on all 50 files that carried the magic.
- Strip sizes are 5-8 bits/pixel on average (E-M1: 13,919,144 bytes for 16.1 Mpx = 6.9 bpp), so the data is
  entropy coded and **not** byte-aligned per row.

## 2. Stream header (verified on 50 files)

Offset 0 of the strip: `00 00 00 00 01 00 00` (7 bytes, constant in every compressed file). In every file the
magic occurs exactly once in the strip (searched for the whole strip of E-M1 and E-P1). Data starts at byte 7 (bit 56)
and is read MSB-first as a bit stream (evidence: the seed words below only make sense MSB-first).

## 3. Row seeds (verified layout, semantics uncertain)

Starting at bit 56 there are two 32-bit big-endian words, then the coded pixels of row 0 from bit 120.
Each word looks like `[3-bit tag][15 zero bits][14-bit field]`, i.e. 12-bit pixel-sized values in the low 12 bits
with zero padding above. Examples (word values): E-M1 `0x200004B8, 0xC0000D42`; E-PL7 `0x00000476, 0x00000E7F`;
PEN-F `0x600004A4, 0x80000C5F`; E-M10 IV `0x00000320, 0xC0000C7A`. The 3-bit tags take values 0..6 and appear random
(not understood: they may be a left-over of the previous field rather than a tag).

Evidence that this is the start of each row for the first rows: in E-M1 further runs of >= 18 zero bits occur at stream
bit 32767 and 32799 (two words 32 bits apart, same layout, 12-bit values 0x366 and 0x73E), i.e. the next pair of
seeds. Distance from the end of row 0's seeds to the next seeds: 32,644 bits for 4,638 pixels (7.04 bits/pixel),
consistent with the file's average. The next 32-bit pair after that is not recognisable, so **only the first two rows
carry these zero-padded seed words** (E-M1: no other `0^18 1` pair at row pitch); later rows are presumably seeded from
the row above and use no raw words. This is a hypothesis; it explains why rows 0 and 1 (no same-colour neighbour above)
are special, and nothing else has been found that does.

Row 0 first-pixel values are suspiciously similar across bodies (first word 800..1214 (TG-5: 4094), second word
3167..3711), which does not look like optical-black data (black level ~255 for the E-M1). Possibly these are
predictor start values rather than pixels. Unresolved.

## 4. Pixel code: what is known and what is not

### 4.1 From the patents (not yet tied to the files)

Both patents describe, for a single-channel raster with neighbours a (left), b (up), c (up-left):

- Predictor (default): median edge detector. Px = max(a,b) if c < min(a,b); min(a,b) if c > max(a,b); else a+b-c.
  Other listed variants: a; b; c; a+b-c; a+(b-c)/2; b+(a-c)/2; (a+b)/2. Line start uses b (or 0 in the worked example).
- Residual e = x - Px. Magnitude `abs = e` if e >= 0 else `-e - 1`, with the sign sent as a separate bit
  (alternative: interleave 0,-1,1,-2...). Position of the sign bit is **not stated**.
- Golomb-Rice: low `k` bits fixed, remaining high part written as zeros then a terminating `1`
  (patent example: abs = 0b00001110110, k = 5 -> `000110110`... that is 3 zeros, `1`, then `10110`).
- `k` from two "flatness" run counters (patent 1: th1 = 15, th2 = 31, run threshold 2, k_low 2 / 4, k_high 5,
  `k = min(k1,k2)`; patent 2: th1 = 3, th2 = 15), computed from the **previous** pixel's error so the decoder can follow.
  Embodiment 2 raises k to `max(bitlength(previous abs), min(k1,k2))`, with 5 for the first pixel of a line. Counters
  reset every line.
- Patent 2 only: LSB of the error is split off and sent raw, the rest is run through a recursive correction filter
  `C_i = floor(P'_i/2) + floor(C_{i-1}/2)` before coding.
- Neither patent mentions Bayer, same-colour neighbours, ORF, headers or row seeds.

### 4.2 Tests against the files (all negative or inconclusive)

1. **Rice with patent k rules.** Decoding row 0 of the E-M1 from bit 120 with (th1,th2) = (15,31) and (3,15), sign
   before / after / absent, embodiment 1 and 2, initial k 0/3/5, left prediction two pixels back (same colour): the
   consumed length never matches the 32,644 bits measured (best error 1,600 bits), and reconstructed values wander
   outside 12 bits. **Not** the patent's embodiment as written, at least not with these predictor/neighbour choices.
2. **Exp-Golomb-like family (`z` ones, `0`, then `z+4` bits).** The parse of 4,638 pixels from bit 120 ends at
   exactly 32,768 (a byte boundary, 4 bits from the measured seed position). **This is NOT evidence:** for random
   bits that parse has mean 2z+5 = 7.0 bits/pixel, which is the file's average rate; the z histogram is exactly
   geometric (2240, 1225, 594, 291, 134...) and the bits after the prefix are uniform in every class (checked per
   class on the first 3 bits). I mistook this for a hit first; the statistics show it is a coincidence of the mean.
   Every residual mapping tried for it (two's complement, sign-magnitude, zig-zag, offset, JPEG-style) drifts by
   tens of thousands of counts within one row.
3. **Run statistics.** Over the first 2 MB of the E-M1 and OM-1 Mk II streams, runs of 0s and of 1s are almost
   geometric(1/2) (first run lengths 0.52, 0.26, 0.12, 0.06, 0.03...) with the longest zero run about 8 and the longest
   one-run about 10. The stream is close to random, as a well-matched adaptive code produces; the slightly lighter tails than random
   (zero runs >= 8 rarer than 1/256) are weak and unexplained.
   Zero runs of 12..39 bits occur in the header and seed words and sparsely elsewhere (E-M1: 396 in 14 MB; they
   cluster at multiples of about one row).
4. Because the bits are near-uniform, simple per-bit statistics cannot find the code. The needed discriminator is
   a decoded-image check (section 7).

### 4.3 What remains open

- The predictor actually used (left-only on row 0? two-pixel stride? MED with same-colour neighbours?), and whether
  the up-neighbour distance is one or two rows.
- The code (Golomb-Rice vs. a class/size code) and where the sign bit sits.
- The `k` / class selection rule and any per-row state.
- Whether pixels are coded in the sensor order or in an interleaved order (for example the two colours of a row
  alternately, or in column pairs).
- Meaning of the 3-bit tag in the seed words; whether later rows have any seed.

## 5. Row structure (partly verified)

- Row 0 occupies 32,644 bits between its seed pair and row 1's seed pair for E-M1 (4,638 coded pixels + 2 seed pixels).
- Rows are **not** byte aligned in general (probe: with row 1's length unknown, 32,768 was a coincidence per 4.2).
- 12-bit vs 14-bit: the 12-bit files (OM-1 Mark II `20mp-12bit`) show the same 7-byte magic; the OM-1 and OM-1 II
  streams do not show the zero-padded seed pair in the first bytes (bytes 7.. are `00 51 80 61`, `01 b4 05 b4`),
  so the seed layout differs for the newer bodies. Open.

## 6. Verified on which files

| Claim | Files |
|---|---|
| 7-byte magic, constant, single occurrence | E-M1, E-P1 (whole strip); first 14 bytes of 50 files in `corpus2/raw` |
| Seed-like words `[3 bit][zeros][12 bit]` at bits 56 and 88 | visible by eye in most of the 50 (E-M1, E-PL7, PEN-F, E-M10 IV, TG-5, E-P2 checked by script); the OM-1 family differs. Not counted systematically |
| Second seed pair at row 1 (E-M1) | Olympus E-M1 only; E-PL7 / PEN-F / E-P2 / TG-5 detection was ambiguous because of zero runs inside data |
| Uniform/geometric bit statistics | E-M1, OM-1 Mark II 12-bit |
| Rice (patent) and Exp-Golomb parse tests | E-M1 row 0 only |

The CC0 files named in the task (`corpus/raw/orf-olympus-em1.orf`, `...em10iii.orf`) are **not in this checkout**
(`corpus/raw` holds only CR3/DNG samples); the same camera models from `corpus2/raw` were used instead
(`Olympus__E-M1__sc000877.orf`, `Olympus__E-M10_Mark_III__PA080192.ORF`). The camera-JPEG oracle comparison was not
reached because no pixel decode was achieved.

## 7. How to finish (recommended next steps)

1. Pick the E-M1 file and treat the problem as search with a real fitness function: decode rows 0..3, reconstruct
   the Bayer values, and require the values to stay inside 0..4095, to be spatially smooth, and to match the
   luminance structure of the embedded JPEG (extract with `exiftool -b -PreviewImage`, downscale the decoded green
   channel, correlate). Row 0/1 alone is not enough: use 20+ rows so that up-neighbour prediction is exercised.
2. Search space to enumerate in a small Rust or Python harness: predictor in {left, up, MED, average} on the
   same-colour (stride 2) neighbours and on the immediate neighbours; sign bit in {before, after, interleaved};
   code in {Golomb-Rice with the patent `k` rules, size-class codes}; coded pixel order (row order, two-column
   groups); seeds as predictor start values.
3. Use the exact row length constraint (row 0 = 32,644 bits for E-M1; measure it for another body with a clean seed pair)
   as a hard filter: any candidate whose row-0 length differs is wrong. This test is cheap and does not need an oracle.
4. Only after one body decodes, repeat for E-M10 III, E-PL7, TG-5 and the OM-1 family; then record the black level
   and white point (maker note `BlackLevel2`, `ImageProcessing`) so the existing level logic in `crates/raw` can be used.
5. Then write the decoder in `crates/raw/src/vendor/orf.rs` with the usual no-panic rules and add the verified
   rule set to this document, replacing sections 4.3 and 5.

Do not treat the sections above as a complete format description.
