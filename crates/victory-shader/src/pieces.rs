//! Graphics pipelines the title hands over in pieces (device method 0x1c8;
//! the compute form, 0x218, is [`crate::native::compute_container`]).
//!
//! A description (0x158 bytes) is a root signature, then (address, size)
//! pairs: the pipeline registers (0x184 bytes, 0x17c without a pixel
//! program), an input layout (semantic names; not read), the pixel program
//! (words 5-8: code in GPU memory, then its metadata) and the vertex program
//! (words 9-12); a depth-only pipeline has no pixel program. The metadata is
//! the `FSPX` kind 7 form at version 2 (records from +0x10).
//!
//! The vertex program fetches its own vertices: it loads a buffer
//! descriptor from a table the title fills with its vertex buffers (16
//! bytes a buffer) and reads each attribute with an `idxen` buffer load at
//! the vertex (or instance) index. Those loads become input-assembler
//! attributes here: the loads are replaced by `s_nop`, the attribute is
//! declared at the load's VGPRs (as a fetch shader would leave it), and the
//! host input layout reads the same bytes (the load's offset and format,
//! the table slot as the input slot). What only served the fetch goes too.
//!
//! Registers (byte offsets in the register piece), as far as read:
//!
//! ```text
//! 0x2c SPI_PS_INPUT_ADDR    0x38 SPI_SHADER_PGM_RSRC1_PS  0x3c RSRC2_PS
//! 0x4c SPI_SHADER_PGM_RSRC1_VS  0x50 RSRC2_VS
//! 0x5c SPI_PS_IN_CONTROL (interpolants; bit 15 wave32)
//! 0x78 CB_TARGET_MASK       0xa0 PA_SU_SC_MODE_CNTL     0xb8 DB_DEPTH_CONTROL
//! 0xc0 DB_STENCILREFMASK    0xc4 DB_STENCILREFMASK_BF   0xc8 DB_STENCIL_CONTROL
//! 0xd0 CB_BLEND0_CONTROL .. 0xec CB_BLEND7_CONTROL
//! ```
//!
//! Pixel attribute n is taken to be vertex parameter n (the linked form;
//! `SPI_PS_INPUT_CNTL` is not read). Render-target and depth formats are not
//! in the pieces: the host pipeline is made for the targets a draw has.

use std::collections::HashSet;

use crate::gfx10::{self, Fields, Instruction, Operand};
use crate::native::{
    HwStage, NativeProgram, PsInput, ResourceType, RootParam, StageIo, Usage, UsedResource,
    VsInput, VsOutput,
};
use crate::{FourCC, ShaderError};

/// `s_nop 0`.
const S_NOP: u32 = 0xBF80_0000;

/// A vertex attribute for the host input layout: semantic `ATTRIB<index>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexElement {
    pub index: u32,
    /// `DXGI_FORMAT`.
    pub format: u32,
    pub slot: u32,
    pub offset: u32,
    pub instance: bool,
}

/// The containers of one graphics pipeline and what the host pipeline
/// needs besides them.
#[derive(Debug, Clone)]
pub struct Graphics {
    pub vertex: Vec<u8>,
    pub pixel: Option<Vec<u8>>,
    pub elements: Vec<VertexElement>,
    /// Bit n: the pixel program exports render target n.
    pub targets: u8,
}

fn reg(regs: &[u8], at: usize) -> u32 {
    regs.get(at..at + 4)
        .map_or(0, |b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
}

/// The register piece's word at `at` (0 past its end).
pub fn register(regs: &[u8], at: usize) -> u32 {
    reg(regs, at)
}

/// The VGPRs `SPI_SHADER_PGM_RSRC1` allows.
fn vgprs(rsrc1: u32, wave64: bool) -> u16 {
    let granules = (rsrc1 & 0x3F) + 1;
    (granules * if wave64 { 4 } else { 8 }).min(256) as u16
}

/// Code cut to the length its metadata states.
fn code<'a>(code: &'a [u8], meta: &[u8]) -> Result<&'a [u8], String> {
    if reg(meta, 0) != 2 {
        return Err(format!("metadata version {:#x}", reg(meta, 0)));
    }
    let dwords = reg(meta, 8) as usize;
    code.get(..dwords * 4)
        .ok_or_else(|| format!("{dwords:#x} code dwords in {:#x} bytes", code.len()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum R {
    S(u8),
    V(u8),
}

fn regs_of(o: &Operand) -> Vec<R> {
    match *o {
        Operand::Scalar { code, dwords } => (0..dwords).map(|i| R::S(code.wrapping_add(i))).collect(),
        Operand::Vector { reg, dwords } => (0..dwords).map(|i| R::V(reg.wrapping_add(i))).collect(),
        _ => Vec::new(),
    }
}

/// Registers an instruction reads and writes; `None` reads: it may read
/// anything (or changes control flow).
fn effects(i: &Instruction, f: &Fields) -> (Option<Vec<R>>, Vec<R>) {
    let all = |ops: &[&Operand]| ops.iter().flat_map(|o| regs_of(o)).collect::<Vec<_>>();
    let name = i.name().unwrap_or("");
    match f {
        Fields::Sop { dst, src, .. } => {
            let w = dst.iter().flat_map(regs_of).collect();
            (Some(src.iter().flat_map(regs_of).collect()), w)
        }
        Fields::Sopp { .. } => {
            if name == "s_endpgm" {
                // Nothing after it: a read of nothing, the scan ends with the program.
                (Some(Vec::new()), Vec::new())
            } else if name.starts_with("s_waitcnt") || name == "s_nop" || name.starts_with("s_inst_prefetch")
                || name.starts_with("s_setprio") || name.starts_with("s_clause")
            {
                (Some(Vec::new()), Vec::new())
            } else {
                (None, Vec::new())
            }
        }
        Fields::Smem { data, base, soffset, .. } => {
            (Some(all(&[base, soffset])), data.iter().flat_map(regs_of).collect())
        }
        Fields::Valu(v) => {
            let mut w: Vec<R> = v.dst.iter().flat_map(regs_of).collect();
            w.extend(v.carry.iter().flat_map(regs_of));
            (Some(v.src.iter().flat_map(regs_of).collect()), w)
        }
        Fields::Buffer { data, addr, rsrc, soffset, .. } => {
            let load = name.contains("load");
            let mut r = all(&[addr, rsrc, soffset]);
            let w = if load {
                data.iter().flat_map(regs_of).collect()
            } else {
                r.extend(data.iter().flat_map(regs_of));
                Vec::new()
            };
            (Some(r), w)
        }
        Fields::Image { data, addr, rsrc, sampler, .. } => {
            let mut r: Vec<R> = addr.iter().flat_map(regs_of).collect();
            r.extend(regs_of(rsrc));
            r.extend(sampler.iter().flat_map(regs_of));
            if name.contains("store") || name.contains("atomic") {
                r.extend(regs_of(data));
                (Some(r), Vec::new())
            } else {
                (Some(r), regs_of(data))
            }
        }
        Fields::Exp { src, .. } => (Some(src.iter().flatten().flat_map(regs_of).collect()), Vec::new()),
        Fields::Vintrp { dst, src, .. } => (Some(regs_of(src)), regs_of(dst)),
        Fields::Ds { dst, addr, data, .. } => {
            let mut r: Vec<R> = addr.iter().flat_map(regs_of).collect();
            r.extend(data.iter().flat_map(regs_of));
            (Some(r), dst.iter().flat_map(regs_of).collect())
        }
        Fields::Flat { dst, addr, data, saddr, .. } => {
            let mut r: Vec<R> = addr.iter().flat_map(regs_of).collect();
            r.extend(data.iter().flat_map(regs_of));
            r.extend(saddr.iter().flat_map(regs_of));
            (Some(r), dst.iter().flat_map(regs_of).collect())
        }
    }
}

/// `DXGI_FORMAT` and DXIL component type (9 f32, 5 u32, 4 i32) of a typed
/// buffer load's format, or of `dwords` raw dwords (`None`).
fn element_format(format: Option<u8>, dwords: u8) -> Option<(u32, u8)> {
    let (f, u, i) = (9, 5, 4);
    Some(match format {
        None => match dwords {
            1 => (42, u),
            2 => (17, u),
            3 => (7, u),
            4 => (3, u),
            _ => return None,
        },
        Some(n) => match n {
            1 => (61, f),
            2 => (63, f),
            5 => (62, u),
            6 => (64, i),
            7 => (56, f),
            8 => (58, f),
            11 => (57, u),
            12 => (59, i),
            13 => (54, f),
            14 => (49, f),
            15 => (51, f),
            18 => (50, u),
            19 => (52, i),
            20 => (42, u),
            21 => (43, i),
            22 => (41, f),
            23 => (35, f),
            24 => (37, f),
            27 => (36, u),
            28 => (38, i),
            29 => (34, f),
            36 => (26, f),
            50 => (24, f),
            54 => (25, u),
            56 => (28, f),
            57 => (31, f),
            60 => (30, u),
            61 => (32, i),
            62 => (17, u),
            63 => (18, i),
            64 => (16, f),
            65 => (11, f),
            66 => (13, f),
            69 => (12, u),
            70 => (14, i),
            71 => (10, f),
            72 => (7, u),
            73 => (8, i),
            74 => (6, f),
            75 => (3, u),
            76 => (4, i),
            77 => (2, f),
            _ => return None,
        },
    })
}

/// An element of the input layout piece: after a header (element count at
/// 0x4c), from 0x50 each a u32 name length, the name (with its 0), then
/// `D3D12_INPUT_ELEMENT_DESC` without the name: semantic index, format
/// (the console's numbering), input slot, byte offset, class, step rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutElement {
    pub format: u32,
    pub slot: u32,
    pub offset: u32,
    pub instance: bool,
}

/// The elements of an input layout piece.
pub fn input_layout(blob: &[u8]) -> Vec<LayoutElement> {
    let count = reg(blob, 0x4c).min(32) as usize;
    let mut at = 0x50;
    let mut out = Vec::new();
    for _ in 0..count {
        let len = reg(blob, at) as usize;
        if len == 0 || len > 64 || at + 4 + len + 24 > blob.len() {
            break;
        }
        let f = at + 4 + len;
        out.push(LayoutElement {
            format: reg(blob, f + 4),
            slot: reg(blob, f + 8),
            offset: reg(blob, f + 12),
            instance: reg(blob, f + 16) == 1,
        });
        at = f + 24;
    }
    out
}

/// The DXIL component type of a `DXGI_FORMAT`'s channels (9 f32, 5 u32, 4 i32).
fn format_comp(f: u32) -> u8 {
    match f {
        3 | 7 | 12 | 17 | 25 | 30 | 36 | 42 | 50 | 57 | 62 => 5,
        4 | 8 | 14 | 18 | 32 | 38 | 43 | 52 | 59 | 64 => 4,
        _ => 9,
    }
}

struct Fetch {
    element: VertexElement,
    vgpr: u8,
    channels: u8,
    comp: u8,
}

/// The vertex program with its fetches removed, and the fetches.
fn unfetch(
    code: &[u8],
    wave64: bool,
    layout: &[LayoutElement],
    reserved: &HashSet<u8>,
) -> Result<(Vec<u8>, Vec<Fetch>), String> {
    // Which user SGPR an SGPR's value was copied from (s_mov), 0xFF unknown.
    let mut alias: std::collections::HashMap<u8, u8> = Default::default();
    let resolve = |alias: &std::collections::HashMap<u8, u8>, s: u8| *alias.get(&s).unwrap_or(&s);
    let ins = gfx10::decode(code).map_err(|e| e.to_string())?;
    let fields: Vec<Fields> = ins.iter().map(|i| i.fields(wave64)).collect();
    let mut gone = vec![false; ins.len()];
    let mut fetched = vec![false; ins.len()];
    // Buffer descriptors s_load_dwordx4 loaded: first SGPR -> table slot.
    let mut descriptors: Vec<(u8, u32)> = Vec::new();
    let mut fetches = Vec::new();
    // The last writer of each VGPR, for the index a load reads.
    let mut writer: std::collections::HashMap<u8, usize> = Default::default();
    for (n, (i, f)) in ins.iter().zip(&fields).enumerate() {
        let name = i.name().unwrap_or("");
        if let Fields::Buffer { data, addr, rsrc, soffset, offset, offen, idxen, format, lds, tfe, .. } = f
            && *idxen
            && (name.starts_with("buffer_load_dword") || name.starts_with("tbuffer_load_format") || name.starts_with("buffer_load_format"))
            && let Operand::Scalar { code: rs, .. } = *rsrc
            && let Some(&(_, slot)) = descriptors.iter().find(|d| d.0 == rs)
            && let Some(el) = layout.iter().find(|e| e.slot == slot && e.offset == u32::from(*offset))
        {
            let soffset_zero = matches!(soffset, Operand::Const(128)) || matches!(soffset, Operand::Scalar { code: gfx10::NULL, .. });
            if *offen || *lds || *tfe || !soffset_zero || name.contains("d16") {
                return Err(format!("a vertex fetch of another form: {}", i.text(wave64)));
            }
            let Some(Operand::Vector { reg, dwords }) = *data else {
                return Err("a vertex fetch without a destination".into());
            };
            let Operand::Vector { reg: index, .. } = *addr else {
                return Err("a vertex fetch at a scalar index".into());
            };
            // v0 the vertex index, v3 the instance index; or a sum with one.
            let source = |r: u8| -> Option<u8> {
                if r == 0 || r == 3 {
                    if !writer.contains_key(&r) {
                        return Some(r);
                    }
                }
                let w = *writer.get(&r)?;
                let Fields::Valu(v) = &fields[w] else { return None };
                v.src.iter().find_map(|s| match *s {
                    Operand::Vector { reg: 0, .. } => Some(0),
                    Operand::Vector { reg: 3, .. } => Some(3),
                    _ => None,
                })
            };
            let instance = match source(index) {
                Some(0) => false,
                Some(3) => true,
                _ => return Err(format!("a vertex fetch at an index of unknown origin: {}", i.text(wave64))),
            };
            let typed = if name.starts_with("buffer_load_format") { Some(el.format) } else { None };
            let (dxgi, comp) = typed
                .map(|f| (f, format_comp(f)))
                .or_else(|| element_format(*format, dwords))
                .ok_or_else(|| format!("a vertex fetch of an unknown format: {}", i.text(wave64)))?;
            let k = fetches.len() as u32;
            fetches.push(Fetch {
                element: VertexElement { index: k, format: dxgi, slot, offset: u32::from(*offset), instance },
                vgpr: reg,
                channels: dwords,
                comp,
            });
            fetched[n] = true;
            gone[n] = true;
            continue;
        }
        let (_, writes) = effects(i, f);
        // A write ends what a descriptor register held.
        descriptors.retain(|&(s, _)| !writes.iter().any(|w| matches!(w, R::S(x) if (s..s + 4).contains(x))));
        // The vertex buffer table: a base copied from a user SGPR that is no
        // root parameter's.
        let table = |base: &Operand| match *base {
            Operand::Scalar { code: b, .. } => {
                let u = resolve(&alias, b);
                u < 32 && !layout.is_empty() && !reserved.contains(&u)
            }
            _ => false,
        };
        if name == "s_load_dwordx4"
            && let Fields::Smem { data: Some(Operand::Scalar { code: d, .. }), offset, soffset, base, .. } = f
            && table(base)
            && (matches!(soffset, Operand::Scalar { code: gfx10::NULL, .. }) || matches!(soffset, Operand::Const(128)))
            && *offset >= 0
            && offset % 16 == 0
        {
            descriptors.push((*d, (*offset / 16) as u32));
        }
        // Copies keep where a value came from; other writes lose it. The
        // table address's high dword masked to 8 bits (s_bfe) still is it.
        match f {
            Fields::Sop { dst: Some(Operand::Scalar { code: d, dwords }), src, .. }
                if name.starts_with("s_mov_b") =>
            {
                let from = match src.first() {
                    Some(Operand::Scalar { code: c, .. }) => Some(*c),
                    _ => None,
                };
                for k in 0..*dwords {
                    let v = from.map_or(0xFF, |c| resolve(&alias, c + k));
                    alias.insert(d + k, v);
                }
            }
            Fields::Sop { dst: Some(Operand::Scalar { code: d, .. }), src, .. }
                if name == "s_bfe_u32" && matches!(src.first(), Some(Operand::Scalar { code: c, .. }) if c == d) => {}
            _ => {
                for w in &writes {
                    if let R::S(s) = w {
                        alias.insert(*s, 0xFF);
                    }
                }
            }
        }
        for w in writes {
            if let R::V(v) = w {
                writer.insert(v, n);
            }
        }
    }
    if fetches.is_empty() {
        return Ok((code.to_vec(), fetches));
    }
    // Writes only the removed fetches read: gone too (descriptor loads, the
    // index sums, the table address). Straight-line code up to the first
    // branch only; after it a register counts as read.
    let removable = |i: &Instruction| {
        let name = i.name().unwrap_or("");
        name.starts_with("s_load_dword")
            || matches!(name, "s_mov_b32" | "s_mov_b64" | "s_bfe_u32" | "v_add_nc_u32" | "v_mov_b32" | "v_add_u32")
    };
    loop {
        let mut changed = false;
        for n in 0..ins.len() {
            if gone[n] || !removable(&ins[n]) {
                continue;
            }
            let (_, writes) = effects(&ins[n], &fields[n]);
            if writes.is_empty() || writes.iter().any(|w| matches!(w, R::S(x) if *x >= 106)) {
                continue;
            }
            let mut live: HashSet<R> = writes.into_iter().collect();
            let mut read = false;
            for m in n + 1..ins.len() {
                if gone[m] {
                    // A removed fetch still overwrites its VGPRs there.
                    if fetched[m] {
                        for w in effects(&ins[m], &fields[m]).1 {
                            live.remove(&w);
                        }
                        if live.is_empty() {
                            break;
                        }
                    }
                    continue;
                }
                let (reads, writes) = effects(&ins[m], &fields[m]);
                match reads {
                    None => {
                        read = !live.is_empty();
                        break;
                    }
                    Some(r) if r.iter().any(|x| live.contains(x)) => {
                        read = true;
                        break;
                    }
                    _ => {}
                }
                for w in writes {
                    live.remove(&w);
                }
                if live.is_empty() {
                    break;
                }
            }
            if !read {
                gone[n] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // The attributes arrive at entry: nothing kept before a fetch may touch
    // its VGPRs.
    for f in (0..ins.len()).filter(|&f| fetched[f]) {
        let dst: HashSet<R> = effects(&ins[f], &fields[f]).1.into_iter().collect();
        for m in (0..f).filter(|&m| !gone[m]) {
            let (reads, writes) = effects(&ins[m], &fields[m]);
            if reads.unwrap_or_default().iter().chain(&writes).any(|r| dst.contains(r)) {
                return Err(format!("{} before a fetch into its VGPRs", ins[m].text(wave64)));
            }
        }
    }
    let mut words: Vec<u32> = code
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    for (i, _) in ins.iter().zip(&gone).filter(|(_, g)| **g) {
        for k in 0..i.words.len() {
            words[i.offset / 4 + k] = S_NOP;
        }
    }
    Ok((words.iter().flat_map(|w| w.to_le_bytes()).collect(), fetches))
}

/// Parameters a vertex program exports (`exp param<n>`: channel mask by n)
/// and whether it exports more than `pos0`.
fn exports(code: &[u8], wave64: bool) -> Result<([u8; 32], u8, bool, [u8; 2], u8), String> {
    // Channels of mrtz: depth, stencil reference, coverage.
    let mut z = 0u8;
    let mut params = [0u8; 32];
    let mut targets = 0u8;
    let mut more = false;
    // Channels of pos1 and pos2: clip distances.
    let mut clip = [0u8; 2];
    for i in gfx10::decode(code).map_err(|e| e.to_string())? {
        if let Fields::Exp { target, src, .. } = i.fields(wave64) {
            let mask = src.iter().enumerate().filter(|(_, s)| s.is_some()).fold(0u8, |m, (c, _)| m | 1 << c);
            match target {
                0..=7 => targets |= 1 << target,
                8 => z |= mask,
                9 => {}
                12 => {}
                13 | 14 => clip[usize::from(target - 13)] |= mask,
                15 => more = true,
                32..=63 => params[usize::from(target - 32)] |= mask,
                _ => more = true,
            }
        }
    }
    Ok((params, targets, more, clip, z))
}

/// Pixel attributes read: (mask by attribute, flat by attribute).
fn attributes(code: &[u8], wave64: bool) -> Result<([u8; 32], u32, bool), String> {
    let mut read = [0u8; 32];
    let mut flat = 0u32;
    let mut half = false;
    for i in gfx10::decode(code).map_err(|e| e.to_string())? {
        let name = i.name().unwrap_or("");
        let attr = match i.fields(wave64) {
            Fields::Vintrp { attr, .. } => Some(attr),
            Fields::Valu(v) if name.starts_with("v_interp") => v.src.iter().copied().find(|s| matches!(s, Operand::Attr { .. })),
            _ => None,
        };
        if let Some(Operand::Attr { index, chan, high }) = attr {
            read[usize::from(index & 31)] |= 1 << (chan & 3);
            if name.starts_with("v_interp_mov") {
                flat |= 1 << (index & 31);
            }
            half |= high || name.contains("f16");
        }
    }
    Ok((read, flat, half))
}

fn element(name: &str, index: u32, kind: u8, comp: u8, row: u32, cols: u8, usage: u8) -> crate::emit::container::SigElement {
    crate::emit::container::SigElement {
        name: name.into(),
        index,
        kind,
        comp,
        interp: 0,
        row: Some(row),
        col: 0,
        cols,
        usage,
        half: false,
    }
}

/// Our stage I/O entry (`FSPX` kind 0x100): u32 stage (4 vertex, 5 pixel),
/// wave64, VGPRs, `SPI_PS_INPUT_ADDR`, then the inputs (count, then a word
/// each: row | mask << 8 | VGPR or attribute << 16 | flat << 24), then the
/// outputs (count, then row | parameter << 8).
fn io_entry(stage: u32, wave64: bool, vgprs: u16, input_addr: u32, inputs: &[u32], outputs: &[u32]) -> Vec<u8> {
    let mut w = vec![stage, u32::from(wave64), u32::from(vgprs), input_addr, inputs.len() as u32];
    w.extend_from_slice(inputs);
    w.push(outputs.len() as u32);
    w.extend_from_slice(outputs);
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn container(code: &[u8], meta: &[u8], io: &[u8], isg1: &[u8], osg1: &[u8]) -> Vec<u8> {
    let blob = crate::native::fspx_blob(&[
        (6, code),
        (7, meta),
        (crate::native::FSPX_STAGE_IO, io),
    ]);
    crate::dxbc::build(&[(FourCC::ACCC, &blob), (FourCC::ISG1, isg1), (FourCC::OSG1, osg1)])
}

/// The containers for a pipeline's pieces: the register piece, the vertex
/// program (code, metadata) and the pixel program if there is one.
pub fn graphics(
    regs: &[u8],
    layout: &[u8],
    vs: (&[u8], &[u8]),
    ps: Option<(&[u8], &[u8])>,
) -> Result<Graphics, String> {
    let vs_wave64 = true;
    let vs_code = code(vs.0, vs.1)?;
    // The user SGPRs the vertex program's root parameters take.
    let (params, used) = crate::native::fspx_resources(vs.1).map_err(|e| e.to_string())?;
    let mut reserved: HashSet<u8> = HashSet::new();
    for p in &params {
        for k in 0..p.sgprs.max(1) {
            reserved.insert(p.user_data.wrapping_add(k as u8));
        }
    }
    for u in used.iter().filter(|u| u.root != crate::native::RootType::ImmediateSampler) {
        reserved.insert(u.user_data);
        reserved.insert(u.user_data.wrapping_add(1));
    }
    let elements = input_layout(layout);
    let (vs_code, fetches) = unfetch(vs_code, vs_wave64, &elements, &reserved)?;
    let (params, _, more, clip, _) = exports(&vs_code, vs_wave64)?;
    if clip.iter().any(|&m| m != 0 && m & (m + 1) != 0) {
        return Err("clip distances in other than the first channels".into());
    }
    let clips = clip[0].count_ones() + clip[1].count_ones();
    if more {
        return Err("a vertex program exporting more than a position and parameters".into());
    }
    let (ps_read, ps_flat, ps_targets, ps_z) = match ps {
        Some((c, m)) => {
            let c = code(c, m)?;
            let wave64 = reg(regs, 0x5c) & 1 << 15 == 0;
            let (read, flat, half) = attributes(c, wave64)?;
            if half {
                return Err("a pixel program reading 16-bit attributes".into());
            }
            let (_, targets, more, _, z) = exports(c, wave64)?;
            if more || z & 8 != 0 {
                return Err("a pixel program exporting more than colors, depth, stencil and coverage".into());
            }
            (read, flat, targets, z)
        }
        None => ([0; 32], 0, 0, 0),
    };
    for (a, &mask) in ps_read.iter().enumerate() {
        if mask != 0 && params[a] == 0 {
            return Err(format!("pixel attribute {a} is no vertex parameter"));
        }
    }
    // Rows: 0 the position, 1 + n parameter n.
    let rows: Vec<(u32, u8)> = (0..32u32).filter(|&p| params[p as usize] != 0).map(|p| (p, params[p as usize])).collect();
    let mut isg1 = Vec::new();
    let mut inputs = Vec::new();
    for f in &fetches {
        let k = f.element.index;
        let mask = ((1u16 << f.channels) - 1) as u8;
        isg1.push(element("ATTRIB", k, 0, f.comp, k, f.channels, mask));
        inputs.push(k | u32::from(mask) << 8 | u32::from(f.vgpr) << 16);
    }
    let mut osg1 = vec![element("SV_Position", 0, 3, 9, 0, 4, 0xF)];
    let mut outputs = Vec::new();
    for &(p, mask) in &rows {
        osg1.push(element("TEXCOORD", p, 0, 9, 1 + p, 4, mask));
        outputs.push((1 + p) | p << 8);
    }
    // shortcut: every pos1 and pos2 channel taken for a clip distance
    // (`PA_CL_VS_OUT_CNTL` is not read); the translator numbers them in order.
    if clips > 0 {
        osg1.push(element("SV_ClipDistance", 0, 6, 9, 33, clips.min(4) as u8, ((1u16 << clips.min(4)) - 1) as u8));
    }
    if clips > 4 {
        osg1.push(element("SV_ClipDistance", 1, 6, 9, 34, (clips - 4) as u8, ((1u16 << (clips - 4)) - 1) as u8));
    }
    let vs_io = io_entry(4, vs_wave64, vgprs(reg(regs, 0x4c), vs_wave64), 0, &inputs, &outputs);
    let vertex = container(
        &vs_code,
        vs.1,
        &vs_io,
        &crate::emit::container::signature(&isg1, true),
        &crate::emit::container::signature(&osg1, false),
    );
    let pixel = match ps {
        None => None,
        Some((c, m)) => {
            let c = code(c, m)?;
            let wave64 = reg(regs, 0x5c) & 1 << 15 == 0;
            let input_addr = reg(regs, 0x2c);
            let position = input_addr >> 8 & 0xF;
            let mut isg1 = vec![element("SV_Position", 0, 3, 9, 0, 4, position as u8)];
            let mut inputs = Vec::new();
            for &(p, _) in &rows {
                let read = ps_read[p as usize];
                isg1.push(element("TEXCOORD", p, 0, 9, 1 + p, 4, read));
                if read != 0 {
                    let flat = ps_flat >> p & 1;
                    inputs.push((1 + p) | u32::from(read) << 8 | p << 16 | flat << 24);
                }
            }
            let mut osg1: Vec<_> = (0..8u32)
                .filter(|n| ps_targets >> n & 1 != 0)
                .map(|n| element("SV_Target", n, 16, 9, n, 4, 0xF))
                .collect();
            // System values without a register.
            for (bit, name, kind, comp) in [(1u8, "SV_Depth", 17u8, 9u8), (2, "SV_StencilRef", 20, 5), (4, "SV_Coverage", 14, 5)] {
                if ps_z & bit != 0 {
                    let mut e = element(name, 0, kind, comp, 0, 1, 1);
                    e.row = None;
                    osg1.push(e);
                }
            }
            let io = io_entry(5, wave64, vgprs(reg(regs, 0x38), wave64), input_addr, &inputs, &[]);
            Some(container(
                c,
                m,
                &io,
                &crate::emit::container::signature(&isg1, true),
                &crate::emit::container::signature(&osg1, false),
            ))
        }
    };
    Ok(Graphics {
        vertex,
        pixel,
        elements: fetches.iter().map(|f| f.element).collect(),
        targets: ps_targets,
    })
}

/// The program an `FSPX` blob with our stage I/O entry holds.
pub(crate) fn graphics_program(
    io: &[u8],
    code: &[u8],
    layout: Vec<RootParam>,
    used: Vec<UsedResource>,
) -> Result<NativeProgram, ShaderError> {
    let bad = || ShaderError::Malformed("our stage I/O entry");
    let w: Vec<u32> = io.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
    let get = |i: usize| w.get(i).copied().ok_or_else(bad);
    let n_in = get(4)? as usize;
    let ins: Vec<u32> = (0..n_in).map(|i| get(5 + i)).collect::<Result<_, _>>()?;
    let n_out = get(5 + n_in)? as usize;
    let outs: Vec<u32> = (0..n_out).map(|i| get(6 + n_in + i)).collect::<Result<_, _>>()?;
    let b = |x: u32, s: u32| (x >> s & 0xFF) as u8;
    let (stage, stage_io) = match get(0)? {
        4 => (
            HwStage::Vs,
            StageIo::Vs {
                inputs: ins
                    .iter()
                    .map(|&x| VsInput { usage: Usage::Generic, row: b(x, 0), mask: b(x, 8), vgpr: b(x, 16) })
                    .collect(),
                outputs: outs
                    .iter()
                    .map(|&x| VsOutput { usage: Usage::Generic, row: b(x, 0), param: b(x, 8), fp16: false, pack2: false })
                    .collect(),
            },
        ),
        5 => (
            HwStage::Ps,
            StageIo::Ps {
                inputs: ins
                    .iter()
                    .map(|&x| PsInput {
                        usage: Usage::Generic,
                        row: b(x, 0),
                        mask: b(x, 8),
                        attr: b(x, 16),
                        flat: b(x, 24) != 0,
                        fp16: false,
                        pack2: false,
                        row2: 0,
                    })
                    .collect(),
                input_addr: get(3)?,
                db_shader_control: 0,
                patches: Vec::new(),
            },
        ),
        _ => return Err(bad()),
    };
    let _ = ResourceType::Srv;
    Ok(NativeProgram {
        stage,
        wave64: get(1)? != 0,
        vgprs: get(2)? as u16,
        layout,
        used,
        io: stage_io,
        code: code.to_vec(),
    })
}
