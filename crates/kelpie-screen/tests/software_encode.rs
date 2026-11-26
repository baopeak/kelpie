//! The helper's encoding path on a machine with no GPU worth the name (a CI runner): the inbox software H.264 encoder fed through the same
//! converter the stream uses, and what comes out made ready for the phone. Windows only.

#![cfg(all(windows, feature = "helper"))]

use kelpie_screen::sps::{self, nal_type, nal_units, NAL_SPS};
use kelpie_screen::win::convert::Converter;
use kelpie_screen::win::d3d::{Device, Readback};
use kelpie_screen::win::encode_mf::{self, Encoder};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// A picture that changes every frame: a bar moving across a gradient, and some text-like stripes.
fn pattern(w: u32, h: u32, frame: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let bar = (x + frame * 12) % w < 40;
            let stripe = y % 12 < 2 && x % 7 < 4;
            let v = if bar { 255 } else if stripe { 20 } else { (x * 255 / w) as u8 as u32 };
            px[i..i + 4].copy_from_slice(&[v as u8, (y * 255 / h) as u8, 128, 255]);
        }
    }
    px
}

// Media Foundation's inbox encoder and a D3D11 device are on every Windows desktop and on `windows-latest`. The CPU converter is always run;
// the video processor too where the device has one.
#[test]
fn the_software_encoder_makes_every_frame_and_each_keyframe_says_no_reordering() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    encode_mf::startup().expect("Media Foundation");
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.expect("DXGI");
    let adapter = unsafe { factory.EnumAdapters1(0) }.expect("an adapter (the basic render driver at least)");
    let d = Device::on(&adapter).expect("a D3D11 device");
    // The same size; and a 1024×768 screen at the software rung a stream starts on there (960×720 at 15 fps), scaled.
    // The CPU converter always, and the video processor too where the graphics have one.
    let paths: &[bool] = if d.video { &[true, false] } else { &[true] };
    println!("device: video {}, warp {}, why not video: {:?}", d.video, d.warp, d.fallback);
    for (&cpu, (src, (w, h), fps, kbps)) in paths.iter().flat_map(|c| [(c, ((640u32, 360u32), (640u32, 360u32), 30u32, 1000u32)), (c, ((1024, 768), (960, 720), 15, 1500))]) {
        println!("{} converter, {}x{} to {w}x{h} at {fps} fps", if cpu { "CPU" } else { "GPU" }, src.0, src.1);
        let tex = d.frame_texture(src.0, src.1).expect("a frame texture");
        let mut conv = Converter::new(&d, src, (w, h), fps, cpu).expect("a converter");
        let mut readback = Readback::new();
        let mut enc = Encoder::open(&d, w, h, fps, kbps, false).expect("the inbox software encoder");
        assert!(!enc.hardware);
        let mut params = sps::Params::default();
        if let Some(header) = &enc.sequence_header {
            params.seed(header);
        }
        let mut out = Vec::new();
        for i in 0..40u32 {
            let px = pattern(src.0, src.1, i);
            unsafe { d.ctx.UpdateSubresource(&tex, 0, None, px.as_ptr() as *const _, src.0 * 4, 0) };
            let nv12 = conv.convert(&d, &mut readback, false, &tex).expect("converted");
            let time = i64::from(i) * 10_000_000 / i64::from(fps);
            assert!(enc.encode(&d, &nv12, time, i == 0).unwrap_or_else(|e| panic!("{w}x{h}: encoded: {e}")), "{w}x{h}: the software encoder takes every frame");
            out.extend(enc.poll().expect("polled"));
        }
        for _ in 0..10 {
            out.extend(enc.poll().expect("polled"));
        }
        assert!(out.len() >= 30, "{w}x{h}: {} frames out of 40", out.len());
        let fixed: Vec<Vec<u8>> = out.iter().filter_map(|e| params.fix(&e.data)).collect();
        assert_eq!(fixed.len(), out.len(), "{w}x{h}: every access unit is usable");
        assert!(sps::is_keyframe(&fixed[0]), "{w}x{h}: the first is a keyframe");
        let first_sps = nal_units(&fixed[0]).into_iter().find(|n| nal_type(n) == NAL_SPS).expect("a keyframe carries its SPS");
        let parsed = sps::parse_sps(first_sps).expect("an SPS that reads");
        assert_eq!((parsed.width, parsed.height, parsed.max_num_reorder_frames), (w, h, Some(0)));
        // A long GOP: no keyframe nobody asked for.
        assert!(fixed[1..].iter().all(|au| !sps::is_keyframe(au)), "{w}x{h}: keyframes only when asked");
    }
}