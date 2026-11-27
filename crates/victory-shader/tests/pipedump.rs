//! Programs the title handed device methods 0x1c8 / 0x218, as a run with
//! `VICTORY_PIPE_DUMP` saved them (no game data in the repository: the
//! folder is named by `VICTORY_PIPE_DUMP` and the test is ignored).
//! `cargo test -p victory-shader --test pipedump -- --ignored --nocapture`

use std::path::Path;

fn words(bytes: &[u8]) -> Vec<u64> {
    bytes.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect()
}

/// The blob a description's pair at word `i` points at.
fn blob(dir: &Path, prefix: &str, i: usize, w: &[u64]) -> Option<Vec<u8>> {
    let name = format!("{prefix}-w{i}-{:x}-{:x}.bin", w[i], w[i + 1]);
    std::fs::read(dir.join(name)).ok()
}

#[test]
#[ignore]
fn dumped_compute_programs_translate() {
    let dir = std::env::var_os("VICTORY_PIPE_DUMP").expect("VICTORY_PIPE_DUMP");
    let dir = Path::new(&dir);
    let (mut ok, mut all) = (0, 0);
    for call in 1..=8 {
        let prefix = format!("218-{call}");
        let Ok(desc) = std::fs::read(dir.join(format!("{prefix}-desc.bin"))) else { continue };
        let w = words(&desc);
        let (Some(regs), Some(hash), Some(code), Some(meta)) = (
            blob(dir, &prefix, 1, &w),
            blob(dir, &prefix, 3, &w),
            blob(dir, &prefix, 5, &w),
            blob(dir, &prefix, 7, &w),
        ) else {
            println!("{prefix}: pieces missing");
            continue;
        };
        all += 1;
        let Ok(c) = victory_shader::native::compute_container(&regs, &hash, &code, &meta) else {
            println!("{prefix}: pieces do not make a container");
            continue;
        };
        match victory_shader::emit::translate(&c) {
            Ok(_) => {
                ok += 1;
                println!("{prefix}: translated");
            }
            Err(e) => println!("{prefix}: {e}"),
        }
    }
    println!("{ok} of {all} translated");
    assert!(all > 0);
}

/// Prints the graphics programs of the first dumped 0x1c8 descriptions:
/// slot A (words 5-8, the pixel shader) and slot B (9-12, the vertex shader).
#[test]
#[ignore]
fn dumped_graphics_programs_disassemble() {
    let dir = std::env::var_os("VICTORY_PIPE_DUMP").expect("VICTORY_PIPE_DUMP");
    let dir = Path::new(&dir);
    let call: usize = std::env::var("PIPE_CALL").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let prefix = format!("1c8-{call}");
    let desc = std::fs::read(dir.join(format!("{prefix}-desc.bin"))).unwrap();
    let w = words(&desc);
    for (name, at) in [("slot A", 5), ("slot B", 9)] {
        let (Some(code), Some(meta)) = (blob(dir, &prefix, at, &w), blob(dir, &prefix, at + 2, &w)) else {
            println!("{name}: absent");
            continue;
        };
        let dwords = u32::from_le_bytes(meta[8..12].try_into().unwrap()) as usize;
        println!("== {name}: {} code bytes, meta {:x?}", dwords * 4, &meta[..16]);
        let code = &code[..dwords * 4];
        match victory_shader::gfx10::decode(code) {
            Ok(ins) => {
                for i in ins {
                    println!("  {}", i.text(true));
                }
            }
            Err(e) => println!("  {e}"),
        }
    }
}

/// Every dumped 0x1c8 pipeline through `pieces::graphics` and the translator.
#[test]
#[ignore]
fn dumped_graphics_programs_translate() {
    let dir = std::env::var_os("VICTORY_PIPE_DUMP").expect("VICTORY_PIPE_DUMP");
    let dir = Path::new(&dir);
    let (mut ok, mut all) = (0, 0);
    let stem = std::env::var("PIPE_PREFIX").unwrap_or("1c8-".into());
    for call in 1..=4096 {
        let prefix = format!("{stem}{call}");
        let Ok(desc) = std::fs::read(dir.join(format!("{prefix}-desc.bin"))) else { continue };
        let w = words(&desc);
        let Some(regs) = blob(dir, &prefix, 1, &w) else { continue };
        let vs = (blob(dir, &prefix, 9, &w), blob(dir, &prefix, 11, &w));
        let ps = (blob(dir, &prefix, 5, &w), blob(dir, &prefix, 7, &w));
        let (Some(vc), Some(vm)) = vs else {
            println!("{prefix}: no vertex program");
            continue;
        };
        all += 1;
        let ps = match &ps {
            (Some(c), Some(m)) => Some((c.as_slice(), m.as_slice())),
            _ => None,
        };
        let g = match victory_shader::pieces::graphics(&regs, &blob(dir, &prefix, 3, &w).unwrap_or_default(), (&vc, &vm), ps) {
            Ok(g) => g,
            Err(e) => {
                println!("{prefix}: {e}");
                continue;
            }
        };
        let v = victory_shader::emit::translate(&g.vertex);
        let p = g.pixel.as_ref().map(|c| victory_shader::emit::translate(c));
        match (&v, &p) {
            (Ok(_), None | Some(Ok(_))) => {
                ok += 1;
                println!("{prefix}: translated; elements {:?}, targets {:#x}", g.elements, g.targets);
            }
            _ => println!(
                "{prefix}: vertex {:?} pixel {:?}",
                v.as_ref().err(),
                p.as_ref().and_then(|p| p.as_ref().err())
            ),
        }
    }
    println!("{ok} of {all} translated");
}

/// The vertex program of dump PIPE_CALL after the fetch rewrite.
#[test]
#[ignore]
fn dumped_vertex_program_rewritten() {
    let dir = std::env::var_os("VICTORY_PIPE_DUMP").expect("VICTORY_PIPE_DUMP");
    let dir = Path::new(&dir);
    let call: usize = std::env::var("PIPE_CALL").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let prefix = format!("1c8-{call}");
    let w = words(&std::fs::read(dir.join(format!("{prefix}-desc.bin"))).unwrap());
    let regs = blob(dir, &prefix, 1, &w).unwrap();
    let (vc, vm) = (blob(dir, &prefix, 9, &w).unwrap(), blob(dir, &prefix, 11, &w).unwrap());
    println!("meta {:02x?}", &vm[..]);
    let g = victory_shader::pieces::graphics(&regs, &blob(dir, &prefix, 3, &w).unwrap_or_default(), (&vc, &vm), None);
    let Ok(g) = g else { println!("{:?}", g.err()); return };
    let c = victory_shader::Container::parse(&g.vertex).unwrap();
    let p = &victory_shader::native_programs(c.part(victory_shader::FourCC::ACCC).unwrap().data).unwrap()[0];
    println!("layout {:?}\nused {:?}", p.layout, p.used);
    for i in victory_shader::gfx10::decode(&p.code).unwrap() {
        println!("  {:4x} {}", i.offset, i.text(true));
    }
}
