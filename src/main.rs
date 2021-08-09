use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use winit_input_helper::WinitInputHelper;

use log::{error};
extern crate env_logger;

use pixels::{
    Error, SurfaceTexture, PixelsBuilder, wgpu
};

use std::io::Read;

const GB_SCREEN_WIDTH : u32 = 160;
const GB_SCREEN_HEIGHT: u32 = 144;
const ROM_MAX: usize = 0x200000;

type Byte = u8;
type Word = u16;
type SByte = i8;
type SWord = i16;

struct CPUState {
    tsc: u64, // counting cycles since reset, not part of actual gb hardware but used for instruction timing
    reg_af: Word,
    reg_bc: Word,
    reg_de: Word,
    reg_hl: Word,
    sp: Word,
    pc: Word,
}

// https://gbdev.gg8.se/files/docs/mirrors/pandocs.html#powerupsequence
fn reset() -> CPUState {
    CPUState {
        tsc: 0,
        reg_af: 0x01B0,
        reg_bc: 0x0013,
        reg_de: 0x00D8,
        reg_hl: 0x014D,
        sp: 0xFFFE,
        pc: 0
    }
}

//  jp   nn        C3 nn nn    16 ---- jump to nn, PC=nn
fn jp(cpu: CPUState, low: Byte, high: Byte) -> CPUState {
    CPUState {
        pc: (high as u16) << 8 | (low as u16),
        tsc: cpu.tsc + 16,
        ..cpu
    }
}

fn main() -> Result<(), Error> {
    env_logger::init();

    // window management
    // -----------------
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
    .with_title("cerboy")
    .build(&event_loop)
    .unwrap();
    let min_size: winit::dpi::LogicalSize<f64> =
    winit::dpi::PhysicalSize::new(GB_SCREEN_WIDTH, GB_SCREEN_HEIGHT)
    .to_logical(window.scale_factor());
    window.set_min_inner_size(Some(min_size));

    let mut input = WinitInputHelper::new();

    // surface
    // -------
    let surface_texture = SurfaceTexture::new(window.inner_size().width, window.inner_size().height, &window);
    let mut pixels = PixelsBuilder::new(GB_SCREEN_WIDTH, GB_SCREEN_HEIGHT, surface_texture)
    .request_adapter_options(wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
    })
    .build()?;

    // rom stuff
    // ---------
    let mut rom: Vec<Byte> = vec![0; ROM_MAX];
    let args: Vec<String> = std::env::args().collect();
    println!("{:?}",args);
    assert_eq!(args.len(), 2, "unexpected number of args (must pass in path to rom)");
    let mut file = match std::fs::File::open(&args[1]) {
        Ok(file) => file,
        Err(file) => panic!("failed to open {}", file)
    };
    file.read(&mut rom).expect("failed to read file into memory");

    // emu loop for testing?
    // todo: probably need to disassemble entire room and implement everything in it instead of running until panic
    loop {

    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::RedrawRequested(_) = event {
            for (i, pixel) in pixels.get_frame().chunks_exact_mut(4).enumerate() {
                let slice = [(i % 2 * 0xff) as u8, (i % GB_SCREEN_WIDTH as usize) as u8, 0x00, 0xff];
                pixel.copy_from_slice(&slice)
            }
            if pixels.render().map_err(|e| error!("pixels.render() has failed: {}", e))
            .is_err() {
                *control_flow = ControlFlow::Exit;
                return;
            }
        }
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                window_id,
            } if window_id == window.id() => *control_flow = ControlFlow::Exit,
            _ => {
                if input.update(&event) {
                    if let Some(size) = input.window_resized() {
                        pixels.resize_surface(size.width, size.height);
                    }
                    window.request_redraw();
                }
            },
        }
    });
}
