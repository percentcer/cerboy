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

const HIGH_MASK: Word = 0xFF00;
const LOW_MASK: Word = 0x00FF;

#[derive(Copy, Clone)]
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
const fn reset() -> CPUState {
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

// GMB 8bit-Loadcommands
// ============================================================================
//   ld   r,r         xx         4 ---- r=r
//   ld   r,n         xx nn      8 ---- r=n
//   ld   r,(HL)      xx         8 ---- r=(HL)
//   ld   (HL),r      7x         8 ---- (HL)=r
//   ld   (HL),n      36 nn     12 ----
//   ld   A,(BC)      0A         8 ----
//   ld   A,(DE)      1A         8 ----
//   ld   A,(nn)      FA        16 ----
//   ld   (BC),A      02         8 ----
//   ld   (DE),A      12         8 ----
//   ld   (nn),A      EA        16 ----
//   ld   A,(FF00+n)  F0 nn     12 ---- read from io-port n (memory FF00+n)
//   ld   (FF00+n),A  E0 nn     12 ---- write to io-port n (memory FF00+n)
//   ld   A,(FF00+C)  F2         8 ---- read from io-port C (memory FF00+C)
//   ld   (FF00+C),A  E2         8 ---- write to io-port C (memory FF00+C)
//   ldi  (HL),A      22         8 ---- (HL)=A, HL=HL+1
//   ldi  A,(HL)      2A         8 ---- A=(HL), HL=HL+1
//   ldd  (HL),A      32         8 ---- (HL)=A, HL=HL-1
//   ldd  A,(HL)      3A         8 ---- A=(HL), HL=HL-1

// GMB 16bit-Loadcommands
// ============================================================================
//   ld   rr,nn       x1 nn nn  12 ---- rr=nn (rr may be BC,DE,HL or SP)
//   ld   SP,HL       F9         8 ---- SP=HL
//   push rr          x5        16 ---- SP=SP-2  (SP)=rr   (rr may be BC,DE,HL,AF)
//   pop  rr          x1        12 (AF) rr=(SP)  SP=SP+2   (rr may be BC,DE,HL,AF)

// GMB 8bit-Arithmetic/logical Commands
// ============================================================================
//   add  A,r         8x         4 z0hc A=A+r
//   add  A,n         C6 nn      8 z0hc A=A+n
//   add  A,(HL)      86         8 z0hc A=A+(HL)
//   adc  A,r         8x         4 z0hc A=A+r+cy
//   adc  A,n         CE nn      8 z0hc A=A+n+cy
//   adc  A,(HL)      8E         8 z0hc A=A+(HL)+cy
//   sub  r           9x         4 z1hc A=A-r
//   sub  n           D6 nn      8 z1hc A=A-n
//   sub  (HL)        96         8 z1hc A=A-(HL)
//   sbc  A,r         9x         4 z1hc A=A-r-cy
//   sbc  A,n         DE nn      8 z1hc A=A-n-cy
//   sbc  A,(HL)      9E         8 z1hc A=A-(HL)-cy
//   and  r           Ax         4 z010 A=A & r
//   and  n           E6 nn      8 z010 A=A & n
//   and  (HL)        A6         8 z010 A=A & (HL)

//   xor  r           Ax         4 z000
// ----------------------------------------------------------------------------
const fn impl_xor_r(cpu: CPUState, reg: Byte) -> CPUState {
    let arg: Word = (reg as Word) << Byte::BITS;
    let reg_af: Word = (cpu.reg_af ^ arg) & HIGH_MASK;
    let reg_af: Word = if reg_af != 0 { reg_af } else {
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

const fn xor_a(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_af >> 8) as Byte;
    impl_xor_r(cpu, reg)
}

const fn xor_b(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_bc >> 8) as Byte;
    impl_xor_r(cpu, reg)
}

const fn xor_c(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_bc & LOW_MASK) as Byte;
    impl_xor_r(cpu, reg)
}

const fn xor_d(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_de >> 8) as Byte;
    impl_xor_r(cpu, reg)
}

const fn xor_e(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_de & LOW_MASK) as Byte;
    impl_xor_r(cpu, reg)
}

const fn xor_h(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_hl >> 8) as Byte;
    impl_xor_r(cpu, reg)
}

const fn xor_l(cpu: CPUState) -> CPUState {
    let reg: Byte = (cpu.reg_hl & LOW_MASK) as Byte;
    impl_xor_r(cpu, reg)
}

//   xor  n           EE nn      8 z000
// ----------------------------------------------------------------------------
const fn xor_d8(cpu: CPUState, d8: Byte) -> CPUState {
    let base: CPUState = impl_xor_r(cpu, d8);
    // additional machine cycle, additional argument
    CPUState{
        pc: base.pc + 1,
        tsc: base.tsc + 4,
        ..base
    }
}

//   xor  (HL)        AE         8 z000
//   or   r           Bx         4 z000 A=A | r
//   or   n           F6 nn      8 z000 A=A | n
//   or   (HL)        B6         8 z000 A=A | (HL)
//   cp   r           Bx         4 z1hc compare A-r
//   cp   n           FE nn      8 z1hc compare A-n
//   cp   (HL)        BE         8 z1hc compare A-(HL)
//   inc  r           xx         4 z0h- r=r+1
//   inc  (HL)        34        12 z0h- (HL)=(HL)+1
//   dec  r           xx         4 z1h- r=r-1
//   dec  (HL)        35        12 z1h- (HL)=(HL)-1
//   daa              27         4 z-0x decimal adjust akku
//   cpl              2F         4 -11- A = A xor FF

// GMB 16bit-Arithmetic/logical Commands
// ============================================================================
//   add  HL,rr     x9           8 -0hc HL = HL+rr     ;rr may be BC,DE,HL,SP
//   inc  rr        x3           8 ---- rr = rr+1      ;rr may be BC,DE,HL,SP
//   dec  rr        xB           8 ---- rr = rr-1      ;rr may be BC,DE,HL,SP
//   add  SP,dd     E8          16 00hc SP = SP +/- dd ;dd is 8bit signed number
//   ld   HL,SP+dd  F8          12 00hc HL = SP +/- dd ;dd is 8bit signed number

// GMB Rotate- und Shift-Commands
// ============================================================================
//   rlca           07           4 000c rotate akku left
//   rla            17           4 000c rotate akku left through carry
//   rrca           0F           4 000c rotate akku right
//   rra            1F           4 000c rotate akku right through carry
//   rlc  r         CB 0x        8 z00c rotate left
//   rlc  (HL)      CB 06       16 z00c rotate left
//   rl   r         CB 1x        8 z00c rotate left through carry
//   rl   (HL)      CB 16       16 z00c rotate left through carry
//   rrc  r         CB 0x        8 z00c rotate right
//   rrc  (HL)      CB 0E       16 z00c rotate right
//   rr   r         CB 1x        8 z00c rotate right through carry
//   rr   (HL)      CB 1E       16 z00c rotate right through carry
//   sla  r         CB 2x        8 z00c shift left arithmetic (b0=0)
//   sla  (HL)      CB 26       16 z00c shift left arithmetic (b0=0)
//   swap r         CB 3x        8 z000 exchange low/hi-nibble
//   swap (HL)      CB 36       16 z000 exchange low/hi-nibble
//   sra  r         CB 2x        8 z00c shift right arithmetic (b7=b7)
//   sra  (HL)      CB 2E       16 z00c shift right arithmetic (b7=b7)
//   srl  r         CB 3x        8 z00c shift right logical (b7=0)
//   srl  (HL)      CB 3E       16 z00c shift right logical (b7=0)

// GMB Singlebit Operation Commands
// ============================================================================
//   bit  n,r       CB xx        8 z01- test bit n
//   bit  n,(HL)    CB xx       12 z01- test bit n
//   set  n,r       CB xx        8 ---- set bit n
//   set  n,(HL)    CB xx       16 ---- set bit n
//   res  n,r       CB xx        8 ---- reset bit n
//   res  n,(HL)    CB xx       16 ---- reset bit n

// GMB CPU-Controlcommands
// ============================================================================
//   ccf            3F           4 -00c cy=cy xor 1
//   scf            37           4 -001 cy=1

//   nop            00           4 ---- no operation
// ----------------------------------------------------------------------------
const fn nop(cpu: CPUState) -> CPUState { 
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 4,
        ..cpu
    }
}

//   halt           76         N*4 ---- halt until interrupt occurs (low power)
//   stop           10 00        ? ---- low power standby mode (VERY low power)
//   di             F3           4 ---- disable interrupts, IME=0
//   ei             FB           4 ---- enable interrupts, IME=1

// GMB Jumpcommands
// ============================================================================

//   jp   nn        C3 nn nn    16 ---- jump to nn, PC=nn
// ----------------------------------------------------------------------------
fn jp(cpu: CPUState, low: Byte, high: Byte) -> CPUState {
    CPUState {
        pc: (high as Word) << Byte::BITS | (low as Word),
        tsc: cpu.tsc + 16,
        ..cpu
    }
}

//   jp   HL        E9           4 ---- jump to HL, PC=HL
//   jp   f,nn      xx nn nn 16;12 ---- conditional jump if nz,z,nc,c
//   jr   PC+dd     18 dd       12 ---- relative jump to nn (PC=PC+/-7bit)
//   jr   f,PC+dd   xx dd     12;8 ---- conditional relative jump if nz,z,nc,c
//   call nn        CD nn nn    24 ---- call to nn, SP=SP-2, (SP)=PC, PC=nn
//   call f,nn      xx nn nn 24;12 ---- conditional call if nz,z,nc,c
//   ret            C9          16 ---- return, PC=(SP), SP=SP+2
//   ret  f         xx        20;8 ---- conditional return if nz,z,nc,c
//   reti           D9          16 ---- return and enable interrupts (IME=1)
//   rst  n         xx          16 ---- call to 00,08,10,18,20,28,30,38

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
                let slice = [(i % 2 * 0xFF) as u8, (i % GB_SCREEN_WIDTH as usize) as u8, 0x00, 0xFF];
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

#[cfg(test)]
mod tests_cpu {
    use super::*;

    const HARNESS: CPUState = reset();

    #[test]
    fn test_impl_xor_r() {
        let result = impl_xor_r(HARNESS, 0x13);
        assert_eq!(result.pc, HARNESS.pc + 1, "incorrect program counter");
        assert_eq!(result.tsc, HARNESS.tsc + 4, "incorrect time stamp counter");
        assert_eq!(result.reg_af, 0x1200, "incorrect value in reg_af (expected 0x{:X} got 0x{:X})", 0x1200, result.reg_af);
    }

    #[test]
    fn test_xor_a() {
        let result = xor_a(HARNESS);
        assert_eq!(result.reg_af, 0x0080);
    }

    #[test]
    fn test_xor_bc() {
        let state = CPUState {
            reg_bc: 0xCD11,
            ..HARNESS
        };
        assert_eq!(xor_b(state).reg_af, 0xCC00);
        assert_eq!(xor_c(state).reg_af, 0x1000);
    }

    #[test]
    fn test_xor_d8() {
        let result = xor_d8(HARNESS, 0xFF);
        assert_eq!(result.pc, HARNESS.pc + 2, "incorrect program counter");
        assert_eq!(result.tsc, HARNESS.tsc + 8, "incorrect time stamp counter");
        assert_eq!(result.reg_af, 0xFF00, "incorrect xor value in reg a");
    }
}
