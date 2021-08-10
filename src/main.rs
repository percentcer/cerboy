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

// https://gbdev.gg8.se/files/docs/mirrors/pandocs.html
//
// CPU          - 8-bit (Similar to the Z80 processor)
// Clock Speed  - 4.194304MHz (4.295454MHz for SGB, max. 8.4MHz for CGB)
// Work RAM     - 8K Byte (32K Byte for CGB)
// Video RAM    - 8K Byte (16K Byte for CGB)
// Screen Size  - 2.6"
// Resolution   - 160x144 (20x18 tiles)
// Max sprites  - Max 40 per screen, 10 per line
// Sprite sizes - 8x8 or 8x16
// Palettes     - 1x4 BG, 2x3 OBJ (for CGB: 8x4 BG, 8x3 OBJ)
// Colors       - 4 grayshades (32768 colors for CGB)
// Horiz Sync   - 9198 KHz (9420 KHz for SGB)
// Vert Sync    - 59.73 Hz (61.17 Hz for SGB)
// Sound        - 4 channels with stereo sound
// Power        - DC6V 0.7W (DC3V 0.7W for GB Pocket, DC3V 0.6W for CGB)
//
// 0000-3FFF   16KB ROM Bank 00     (in cartridge, fixed at bank 00)
// 4000-7FFF   16KB ROM Bank 01..NN (in cartridge, switchable bank number)
// 8000-9FFF   8KB Video RAM (VRAM) (switchable bank 0-1 in CGB Mode)
// A000-BFFF   8KB External RAM     (in cartridge, switchable bank, if any)
// C000-CFFF   4KB Work RAM Bank 0 (WRAM)
// D000-DFFF   4KB Work RAM Bank 1 (WRAM)  (switchable bank 1-7 in CGB Mode)
// E000-FDFF   Same as C000-DDFF (ECHO)    (typically not used)
// FE00-FE9F   Sprite Attribute Table (OAM)
// FEA0-FEFF   Not Usable
// FF00-FF7F   I/O Ports
// FF80-FFFE   High RAM (HRAM)
// FFFF        Interrupt Enable Register

const GB_SCREEN_WIDTH : u32 = 160;
const GB_SCREEN_HEIGHT: u32 = 144;
const ROM_MAX: usize = 0x200000;

type Byte = u8;
type Word = u16;
type SByte = i8;
type SWord = i16;

const HIGH_MASK: Word = 0xff00;
const LOW_MASK: Word = 0x00ff;

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

fn nop(cpu: CPUState) -> CPUState { 
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 4,
        ..cpu
    }
}

fn impl_xor_r(cpu: CPUState, reg: Byte) -> CPUState {
    let arg: Word = (reg as Word) << 8;
    let reg_af: Word = (cpu.reg_af ^ arg) & HIGH_MASK;
    let reg_af: Word = if reg_af == 0 { reg_af } else {
        // flags
        // Z N H C
        // 1 0 0 0
        reg_af ^ 0x0080
    };
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 4,
        reg_af: reg_af,
        ..cpu
    }
}

fn xor_a(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_af >> 8) as Byte;
    impl_xor_r(cpu, reg)
}

fn xor_b(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_bc >> 8) as Byte;
    impl_xor_r(cpu, reg)
}

fn xor_c(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_bc & LOW_MASK) as Byte;
    impl_xor_r(cpu, reg)
}

fn xor_d(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_de >> 8) as Byte;
    impl_xor_r(cpu, reg)
}

fn xor_e(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_de & LOW_MASK) as Byte;
    impl_xor_r(cpu, reg)
}

fn xor_h(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_hl >> 8) as Byte;
    impl_xor_r(cpu, reg)
}

fn xor_l(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_hl & LOW_MASK) as Byte;
    impl_xor_r(cpu, reg)
}

fn xor_d8(cpu: CPUState, d8: Byte) -> CPUState {
    let base: CPUState = impl_xor_r(cpu, d8);
    // additional machine cycle, additional argument
    CPUState{
        pc: base.pc + 1,
        tsc: base.tsc + 4,
        ..base
    }
}

// todo xor_hl which requires system memory

//  jp   nn        C3 nn nn    16 ---- jump to nn, PC=nn
fn jp(cpu: CPUState, low: Byte, high: Byte) -> CPUState {
    CPUState {
        pc: (high as Word) << 8 | (low as Word),
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
