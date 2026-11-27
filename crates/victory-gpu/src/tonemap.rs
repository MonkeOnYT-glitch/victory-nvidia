//! A tone mapping of our own in place of the title's (`VICTORY_TONEMAP=1`,
//! off by default).
//!
//! What the target dumps of the prison yard's window shot show (runs gb1,
//! gb3, 2026-11-27): the title's HDR picture before its tone mapping is
//! smooth and lit (the cell's walls hold their light), and its tone-mapping
//! pass (pixel program `6318f669e4f17d65`, a full-screen triangle into a
//! 1920x1080 R10G10B10A2 target: exposure, local exposure, three 33^3
//! colour-grading tables) turns it into a picture whose dark half is black
//! and whose gradients fall into contour bands, whatever the lighting.
//!
//! Right after that draw, two draws of ours write the same target again
//! from the same HDR picture (the title's own view of it, read through its
//! table): one pixel that measures the picture's mean log luminance and
//! moves the exposure towards it over time (kept in a buffer of ours), then
//! the picture through that exposure, a filmic curve (the ACES fit), a warm
//! grade, display gamma and a dither below a step of the target. The
//! title's interface is composited after it, as before.
//!
//! `VICTORY_TONEMAP_PS=<hash>[,<hash>]`: the title's tone-mapping pixel
//! programs (the one above by default). `VICTORY_TONEMAP_KEY`: the mean
//! the exposure aims for (0.10). `VICTORY_TONEMAP_WARM`: how warm (0).
//! `VICTORY_TONEMAP_SATURATION` (0.85: the HDR picture of the sunset is very
//! orange). A negative key: that fixed scale, no measurement (a check).

use std::sync::OnceLock;

use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

use crate::host::{HostList, host};

/// Where the tone-mapping program reads the HDR picture (root parameter,
/// dwords into its table): `Srv s1+304` in the frame listings.
pub(crate) const HDR: (u8, u32) = (1, 304);

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=8, b0), DescriptorTable(SRV(t0)), UAV(u0)"
cbuffer C : register(b0) { uint width; uint height; float key; float follow; float warm; uint frame; float pad0; float pad1; }
Texture2D<float4> hdr : register(t0);
RWByteAddressBuffer state : register(u0);
static const float3 LUMA = float3(0.2126, 0.7152, 0.0722);

[RootSignature(RS)]
float4 vs(uint id : SV_VertexID) : SV_Position {
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
}

// One pixel: the picture's mean log luminance on a 96x54 grid, followed in
// time (the first frame takes it whole).
[RootSignature(RS)]
float4 measure(float4 pos : SV_Position) : SV_Target {
    float sum = 0.0;
    for (uint y = 0; y < 54; y++) {
        for (uint x = 0; x < 96; x++) {
            int2 p = int2((x + 0.5) * width / 96.0, (y + 0.5) * height / 54.0);
            float l = dot(max(hdr.Load(int3(p, 0)).rgb, 0.0), LUMA);
            // (Half floats hold infinities, the sun, and NaNs: bounded, left out.)
            if (!(l >= 0.0)) l = 0.0;
            sum += log(clamp(l, 1e-5, 1e4));
        }
    }
    float now = sum / (96.0 * 54.0);
    float was = asfloat(state.Load(0));
    uint seen = state.Load(4);
    float kept = seen == 0x7e57 && !isnan(was) && !isinf(was) ? lerp(was, now, follow) : now;
    state.Store(0, asuint(kept));
    state.Store(4, 0x7e57);
    return 0;
}

float3 aces(float3 x) {
    return saturate((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14));
}

[RootSignature(RS)]
float4 tone(float4 pos : SV_Position) : SV_Target {
    int2 p = int2(pos.xy);
    float3 c = hdr.Load(int3(p, 0)).rgb;
    c = float3(c.r >= 0.0 ? min(c.r, 6e4) : 0.0, c.g >= 0.0 ? min(c.g, 6e4) : 0.0, c.b >= 0.0 ? min(c.b, 6e4) : 0.0);
    float mean = exp(asfloat(state.Load(0)));
    // A negative key: that fixed scale, no measurement (a check that the
    // picture is read).
    float exposure = key < 0.0 ? -key : clamp(key / max(mean, 1e-6), 1e-4, 1e6);
    c = aces(c * exposure);
    c *= float3(1.0 + warm, 1.0, 1.0 - 1.5 * warm);
    float l = dot(c, LUMA);
    c = saturate(lerp(l.xxx, c, pad0));
    c = pow(c, 1.0 / 2.2);
    // A dither under one step of a 10-bit target (and of the 8-bit one after).
    float n = frac(52.9829189 * frac(dot(float2(p) + frame * 5.588238, float2(0.06711056, 0.00583715))));
    c += (n - 0.5) / 255.0;
    return float4(saturate(c), 1.0);
}
"#;

struct Pass {
    root: ID3D12RootSignature,
    measure: ID3D12PipelineState,
    tone: ID3D12PipelineState,
    state: ID3D12Resource,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Pass {}
// SAFETY: as above.
unsafe impl Sync for Pass {}

/// Whether it is on (`VICTORY_TONEMAP=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TONEMAP").is_some_and(|v| v == "1"))
}

/// Whether `hash` is a tone-mapping pixel program of the title's.
pub(crate) fn is_tonemap(hash: &str) -> bool {
    static HASHES: OnceLock<Vec<String>> = OnceLock::new();
    HASHES
        .get_or_init(|| {
            std::env::var("VICTORY_TONEMAP_PS")
                .unwrap_or_else(|_| "6318f669e4f17d65".to_owned())
                .split(',')
                .map(|h| h.trim().to_owned())
                .filter(|h| !h.is_empty())
                .collect()
        })
        .iter()
        .any(|h| h == hash)
}

fn number(name: &str, default: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).filter(|v: &f32| v.is_finite()).unwrap_or(default)
}

/// The pipelines for a target of `format`.
fn pass(format: DXGI_FORMAT) -> Option<&'static Pass> {
    static PASSES: std::sync::Mutex<Vec<(i32, &'static Pass)>> = std::sync::Mutex::new(Vec::new());
    let mut passes = PASSES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, p)) = passes.iter().find(|(f, _)| *f == format.0) {
        return Some(p);
    }
    let host = host()?;
    let made = (|| -> windows::core::Result<Pass> {
        let vs = crate::host::compile(SOURCE, s!("vs"), s!("vs_5_1"))?;
        let measure = crate::host::compile(SOURCE, s!("measure"), s!("ps_5_1"))?;
        let tone = crate::host::compile(SOURCE, s!("tone"), s!("ps_5_1"))?;
        // SAFETY: the root signature is in the shaders; plain creations.
        unsafe {
            let root: ID3D12RootSignature = host.device.CreateRootSignature(0, &vs)?;
            let pipeline = |ps: &[u8]| -> windows::core::Result<ID3D12PipelineState> {
                let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    VS: D3D12_SHADER_BYTECODE { pShaderBytecode: vs.as_ptr().cast(), BytecodeLength: vs.len() },
                    PS: D3D12_SHADER_BYTECODE { pShaderBytecode: ps.as_ptr().cast(), BytecodeLength: ps.len() },
                    SampleMask: u32::MAX,
                    RasterizerState: D3D12_RASTERIZER_DESC {
                        FillMode: D3D12_FILL_MODE_SOLID,
                        CullMode: D3D12_CULL_MODE_NONE,
                        DepthClipEnable: true.into(),
                        ..Default::default()
                    },
                    PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                    NumRenderTargets: 1,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    ..Default::default()
                };
                desc.RTVFormats[0] = format;
                desc.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
                host.device.CreateGraphicsPipelineState(&desc)
            };
            Ok(Pass {
                measure: pipeline(&measure)?,
                tone: pipeline(&tone)?,
                state: host.uav_buffer(256).ok_or_else(windows::core::Error::empty)?,
                root,
            })
        }
    })();
    match made {
        Ok(p) => {
            tracing::info!(format = format.0, "our tone mapping made (VICTORY_TONEMAP)");
            let p: &'static Pass = Box::leak(Box::new(p));
            passes.push((format.0, p));
            Some(p)
        }
        Err(e) => {
            tracing::warn!(error = %e, format = format.0, "no tone mapping of ours");
            None
        }
    }
}

/// After the title's tone-mapping draw into a target of `format` and
/// `size`, with its view heap set and `hdr` its view of the HDR picture:
/// our two draws over it. The caller sets the title's state again.
pub(crate) fn after(list: &HostList, format: DXGI_FORMAT, size: [u32; 2], hdr: D3D12_GPU_DESCRIPTOR_HANDLE) -> bool {
    let Some(pass) = pass(format) else {
        return false;
    };
    static FRAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let frame = FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let floats = [
        f32::from_bits(size[0]),
        f32::from_bits(size[1]),
        number("VICTORY_TONEMAP_KEY", 0.10),
        number("VICTORY_TONEMAP_FOLLOW", 0.05),
        number("VICTORY_TONEMAP_WARM", 0.0),
        f32::from_bits(frame),
        number("VICTORY_TONEMAP_SATURATION", 0.85),
        0.0,
    ];
    let words: Vec<u32> = floats.iter().map(|f| f.to_bits()).collect();
    list.keep(&pass.state);
    let l = &list.list;
    let full = D3D12_VIEWPORT { Width: size[0] as f32, Height: size[1] as f32, MaxDepth: 1.0, ..Default::default() };
    let one = D3D12_VIEWPORT { Width: 1.0, Height: 1.0, MaxDepth: 1.0, ..Default::default() };
    let rect = windows::Win32::Foundation::RECT { left: 0, top: 0, right: size[0] as i32, bottom: size[1] as i32 };
    // SAFETY: an open list with the title's view heap and target set; the
    // title's draw just read `hdr` in a pixel program.
    unsafe {
        l.SetGraphicsRootSignature(&pass.root);
        l.SetGraphicsRoot32BitConstants(0, 8, words.as_ptr().cast(), 0);
        l.SetGraphicsRootDescriptorTable(1, hdr);
        l.SetGraphicsRootUnorderedAccessView(2, pass.state.GetGPUVirtualAddress());
        l.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        l.RSSetScissorRects(&[rect]);
        l.RSSetViewports(&[one]);
        l.SetPipelineState(&pass.measure);
        l.DrawInstanced(3, 1, 0, 0);
        let barrier = D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_UAV, ..Default::default() };
        l.ResourceBarrier(&[barrier]);
        l.RSSetViewports(&[full]);
        l.SetPipelineState(&pass.tone);
        l.DrawInstanced(3, 1, 0, 0);
    }
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::info!(?size, format = format.0, "our tone mapping in place of the title's");
    }
    true
}
