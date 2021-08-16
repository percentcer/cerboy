extern crate minifb;
use minifb::{Key, Window, WindowOptions};

use log::{error};
extern crate env_logger;

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

const GB_SCREEN_WIDTH : usize = 160;
const GB_SCREEN_HEIGHT: usize = 144;
const ROM_MAX: usize = 0x200000;
const MEM_SIZE: usize = 0xFFFF + 1;

type Byte = u8;
type Word = u16;
type SByte = i8;
type SWord = i16;

const HIGH_MASK: Word = 0xFF00;
const LOW_MASK: Word = 0x00FF;

const FL_Z: Byte = 1 << 7;
const FL_N: Byte = 1 << 6;
const FL_H: Byte = 1 << 5;
const FL_C: Byte = 1 << 4;

// indices
const REG_B: usize = 0;
const REG_C: usize = 1;
const REG_D: usize = 2;
const REG_E: usize = 3;
const REG_H: usize = 4;
const REG_L: usize = 5;
const FLAGS: usize = 6;
const REG_A: usize = 7;

#[derive(Copy, Clone, Debug)]
struct CPUState {
    tsc: u64, // counting cycles since reset, not part of actual gb hardware but used for instruction timing
    reg: [Byte;8],
    sp: Word,
    pc: Word,
}

impl CPUState {
    /// Initializes a new CPUState struct
    /// 
    /// Starting values should match original gb hardware, more here:
    /// https://gbdev.gg8.se/files/docs/mirrors/pandocs.html#powerupsequence
    const fn new() -> CPUState {
        CPUState {
            tsc: 0,
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x13, 0x00, 0xD8, 0x01, 0x4D, 0xB0, 0x01],
            sp: 0xFFFE,
            pc: 0
        }
    }

    /// Commonly used for addresses
    /// 
    /// Combines the H and L registers into a usize for mem indexing
    const fn HL(&self) -> usize {
        combine(self.reg[REG_H], self.reg[REG_L]) as usize
    }
}

fn init_mem() -> Vec<Byte> {
    let mut mem = vec![0;MEM_SIZE];
    mem[0xFF05] = 0x00; //TIMA
    mem[0xFF06] = 0x00; //TMA
    mem[0xFF07] = 0x00; //TAC
    mem[0xFF10] = 0x80; //NR10
    mem[0xFF11] = 0xBF; //NR11
    mem[0xFF12] = 0xF3; //NR12
    mem[0xFF14] = 0xBF; //NR14
    mem[0xFF16] = 0x3F; //NR21
    mem[0xFF17] = 0x00; //NR22
    mem[0xFF19] = 0xBF; //NR24
    mem[0xFF1A] = 0x7F; //NR30
    mem[0xFF1B] = 0xFF; //NR31
    mem[0xFF1C] = 0x9F; //NR32
    mem[0xFF1E] = 0xBF; //NR33
    mem[0xFF20] = 0xFF; //NR41
    mem[0xFF21] = 0x00; //NR42
    mem[0xFF22] = 0x00; //NR43
    mem[0xFF23] = 0xBF; //NR30
    mem[0xFF24] = 0x77; //NR50
    mem[0xFF25] = 0xF3; //NR51
    mem[0xFF26] = 0xF1; // NR52 (note: $F0 on Super GB)
    mem[0xFF40] = 0x91; // LCDC
    mem[0xFF42] = 0x00; // SCY
    mem[0xFF43] = 0x00; // SCX
    mem[0xFF45] = 0x00; // LYC
    mem[0xFF47] = 0xFC; // BGP
    mem[0xFF48] = 0xFF; // OBP0
    mem[0xFF49] = 0xFF; // OBP1
    mem[0xFF4A] = 0x00; // WY
    mem[0xFF4B] = 0x00; // WX
    mem[0xFFFF] = 0x00; // IE
    mem
}

const fn hi(reg: Word) -> Byte { (reg >> Byte::BITS) as Byte }
const fn lo(reg: Word) -> Byte { (reg & LOW_MASK) as Byte }
const fn combine(high: Byte, low: Byte) -> Word {
    (high as Word) << Byte::BITS | (low as Word)
}

// GMB 8bit-Loadcommands
// ============================================================================
//   ld   r,r         xx         4 ---- r=r
// ----------------------------------------------------------------------------
const fn impl_ld_r_d8(cpu: CPUState, dst: usize, val: Byte) -> CPUState {
    let mut reg = cpu.reg;
    reg[dst] = val;
    CPUState{
        pc: cpu.pc+1, 
        tsc: cpu.tsc+4,
        reg,
        ..cpu}
}
fn impl_ld_HL_d8(cpu: CPUState, mem: &mut Vec<Byte>, val: Byte) -> CPUState {
    mem[cpu.HL()] = val;
    CPUState { pc: cpu.pc + 1, tsc: cpu.tsc + 8, ..cpu }
}

// todo: the index arguments could be extracted from the opcode
const fn ld_b_b(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_B]) }
const fn ld_b_c(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_C]) }
const fn ld_b_d(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_D]) }
const fn ld_b_e(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_E]) }
const fn ld_b_h(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_H]) }
const fn ld_b_l(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_L]) }
const fn ld_b_a(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_A]) }

const fn ld_c_b(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_B]) }
const fn ld_c_c(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_C]) }
const fn ld_c_d(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_D]) }
const fn ld_c_e(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_E]) }
const fn ld_c_h(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_H]) }
const fn ld_c_l(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_L]) }
const fn ld_c_a(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_A]) }

const fn ld_d_b(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_B]) }
const fn ld_d_c(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_C]) }
const fn ld_d_d(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_D]) }
const fn ld_d_e(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_E]) }
const fn ld_d_h(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_H]) }
const fn ld_d_l(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_L]) }
const fn ld_d_a(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_A]) }

const fn ld_e_b(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_B]) }
const fn ld_e_c(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_C]) }
const fn ld_e_d(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_D]) }
const fn ld_e_e(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_E]) }
const fn ld_e_h(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_H]) }
const fn ld_e_l(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_L]) }
const fn ld_e_a(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_A]) }

const fn ld_h_b(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_B]) }
const fn ld_h_c(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_C]) }
const fn ld_h_d(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_D]) }
const fn ld_h_e(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_E]) }
const fn ld_h_h(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_H]) }
const fn ld_h_l(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_L]) }
const fn ld_h_a(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_A]) }

const fn ld_l_b(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_B]) }
const fn ld_l_c(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_C]) }
const fn ld_l_d(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_D]) }
const fn ld_l_e(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_E]) }
const fn ld_l_h(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_H]) }
const fn ld_l_l(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_L]) }
const fn ld_l_a(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_A]) }

const fn ld_a_b(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_B]) }
const fn ld_a_c(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_C]) }
const fn ld_a_d(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_D]) }
const fn ld_a_e(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_E]) }
const fn ld_a_h(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_H]) }
const fn ld_a_l(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_L]) }
const fn ld_a_a(cpu: CPUState) -> CPUState { impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_A]) }

//   ld   r,n         xx nn      8 ---- r=n
// ----------------------------------------------------------------------------
const fn ld_b_d8(cpu: CPUState, d8: Byte) -> CPUState { impl_ld_r_d8(cpu, REG_B, d8) }
const fn ld_c_d8(cpu: CPUState, d8: Byte) -> CPUState { impl_ld_r_d8(cpu, REG_C, d8) }
const fn ld_d_d8(cpu: CPUState, d8: Byte) -> CPUState { impl_ld_r_d8(cpu, REG_D, d8) }
const fn ld_e_d8(cpu: CPUState, d8: Byte) -> CPUState { impl_ld_r_d8(cpu, REG_E, d8) }
const fn ld_h_d8(cpu: CPUState, d8: Byte) -> CPUState { impl_ld_r_d8(cpu, REG_H, d8) }
const fn ld_l_d8(cpu: CPUState, d8: Byte) -> CPUState { impl_ld_r_d8(cpu, REG_L, d8) }
const fn ld_a_d8(cpu: CPUState, d8: Byte) -> CPUState { impl_ld_r_d8(cpu, REG_A, d8) }

//   ld   r,(HL)      xx         8 ---- r=(HL)

//   ld   (HL),r      7x         8 ---- (HL)=r
// ----------------------------------------------------------------------------
fn ld_HL_b(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState { impl_ld_HL_d8(cpu, mem, cpu.reg[REG_B]) }
fn ld_HL_c(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState { impl_ld_HL_d8(cpu, mem, cpu.reg[REG_C]) }
fn ld_HL_d(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState { impl_ld_HL_d8(cpu, mem, cpu.reg[REG_D]) }
fn ld_HL_e(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState { impl_ld_HL_d8(cpu, mem, cpu.reg[REG_E]) }
fn ld_HL_h(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState { impl_ld_HL_d8(cpu, mem, cpu.reg[REG_H]) }
fn ld_HL_l(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState { impl_ld_HL_d8(cpu, mem, cpu.reg[REG_L]) }
fn ld_HL_a(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState { impl_ld_HL_d8(cpu, mem, cpu.reg[REG_A]) }

//   ld   (HL),n      36 nn     12 ----
// ----------------------------------------------------------------------------
fn ld_HL_d8(cpu: CPUState, mem: &mut Vec<Byte>, val: Byte) -> CPUState { 
    CPUState {
        pc: cpu.pc + 2,
        tsc: cpu.tsc + 12,
        ..impl_ld_HL_d8(cpu, mem, val)
    }
}

//   ld   A,(BC)      0A         8 ----
// ----------------------------------------------------------------------------
const fn ld_a_BC(cpu: CPUState, mem: &[Byte]) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_A] = mem[combine(reg[REG_B], reg[REG_C]) as usize];
    CPUState { pc: cpu.pc + 1, tsc: cpu.tsc + 8, reg, ..cpu }
}

//   ld   A,(DE)      1A         8 ----
//   ld   A,(nn)      FA        16 ----
// ----------------------------------------------------------------------------
const fn ld_a_DE(cpu: CPUState, mem: &[Byte]) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_A] = mem[combine(reg[REG_D], reg[REG_E]) as usize];
    CPUState { pc: cpu.pc + 1, tsc: cpu.tsc + 8, reg, ..cpu }
}

//   ld   A,(nn)      FA nn nn        16 ----

//   ld   (BC),A      02         8 ----
// ----------------------------------------------------------------------------
fn ld_BC_a(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState {
    let addr = combine(cpu.reg[REG_B], cpu.reg[REG_C]) as usize;
    mem[addr] = cpu.reg[REG_A];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        ..cpu
    }
}

//   ld   (DE),A      12         8 ----
//   ld   (nn),A      EA        16 ----
// ----------------------------------------------------------------------------
fn ld_DE_a(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState {
    let addr = combine(cpu.reg[REG_D], cpu.reg[REG_E]) as usize;
    mem[addr] = cpu.reg[REG_A];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        ..cpu
    }
}

//   ld   A,(FF00+n)  F0 nn     12 ---- read from io-port n (memory FF00+n)
// ----------------------------------------------------------------------------
const fn ld_a_FF00_A8(cpu: CPUState, mem: &[Byte], off: Byte) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_A] = mem[(0xFF00 + off as Word) as usize];
    CPUState { pc: cpu.pc + 2, tsc: cpu.tsc + 12, reg, ..cpu }
}

//   ld   (FF00+n),A  E0 nn     12 ---- write to io-port n (memory FF00+n)
// ----------------------------------------------------------------------------
fn ld_FF00_A8_a(cpu: CPUState, mem: &mut Vec<Byte>, off: Byte) -> CPUState {
    mem[(0xFF00 + off as Word) as usize] = cpu.reg[REG_A];
    CPUState { pc: cpu.pc + 2, tsc: cpu.tsc + 12, ..cpu }
}

//   ld   A,(FF00+C)  F2         8 ---- read from io-port C (memory FF00+C)
// ----------------------------------------------------------------------------
const fn ld_a_FF00_C(cpu: CPUState, mem: &[Byte]) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_A] = mem[(0xFF00 + reg[REG_C] as Word) as usize];
    CPUState { pc: cpu.pc + 1, tsc: cpu.tsc + 8, reg, ..cpu }
}

//   ld   (FF00+C),A  E2         8 ---- write to io-port C (memory FF00+C)
// ----------------------------------------------------------------------------
fn ld_FF00_C_a(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState {
    mem[(0xFF00 + cpu.reg[REG_C] as Word) as usize] = cpu.reg[REG_A];
    CPUState { pc: cpu.pc + 1, tsc: cpu.tsc + 8, ..cpu }
}

//   ldi  (HL),A      22         8 ---- (HL)=A, HL=HL+1
// ----------------------------------------------------------------------------
fn ldi(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState {
    let mut reg = cpu.reg;
    let hl = combine(reg[REG_H], reg[REG_L]);
    let (hli, _) = hl.overflowing_add(1);
    mem[hl as usize] = reg[REG_A];
    reg[REG_H] = hi(hli);
    reg[REG_L] = lo(hli);
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        reg,
        ..cpu
    }
}

//   ldi  A,(HL)      2A         8 ---- A=(HL), HL=HL+1
//   ldd  (HL),A      32         8 ---- (HL)=A, HL=HL-1
// ----------------------------------------------------------------------------
fn ldd(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState {
    let mut reg = cpu.reg;
    let hl = combine(reg[REG_H], reg[REG_L]);
    let (hld, _) = hl.overflowing_sub(1);
    mem[hl as usize] = reg[REG_A];
    reg[REG_H] = hi(hld);
    reg[REG_L] = lo(hld);
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        reg,
        ..cpu
    }
}

//   ldd  A,(HL)      3A         8 ---- A=(HL), HL=HL-1

// GMB 16bit-Loadcommands
// ============================================================================
const fn impl_ld_rr_d16(cpu: CPUState, reg_high: usize, reg_low: usize, high: Byte, low: Byte) -> CPUState {
    let mut reg = cpu.reg;
    reg[reg_high] = high;
    reg[reg_low] = low;
    CPUState {
        pc: cpu.pc + 3,
        tsc: cpu.tsc + 12,
        reg,
        ..cpu
    }
}

//   ld   rr,nn       x1 nn nn  12 ---- rr=nn (rr may be BC,DE,HL or SP)
// ----------------------------------------------------------------------------
const fn ld_bc_d16(cpu: CPUState, low: Byte, high: Byte) -> CPUState { impl_ld_rr_d16(cpu, REG_B, REG_C, high, low) }
const fn ld_de_d16(cpu: CPUState, low: Byte, high: Byte) -> CPUState { impl_ld_rr_d16(cpu, REG_D, REG_E, high, low) }
const fn ld_hl_d16(cpu: CPUState, low: Byte, high: Byte) -> CPUState { impl_ld_rr_d16(cpu, REG_H, REG_L, high, low) }
const fn ld_sp_d16(cpu: CPUState, low: Byte, high: Byte) -> CPUState { 
    CPUState {
        pc: cpu.pc + 3,
        tsc: cpu.tsc + 12,
        sp: combine(high, low),
        ..cpu
    }
}

//   ld   SP,HL       F9         8 ---- SP=HL
//   push rr          x5        16 ---- SP=SP-2  (SP)=rr   (rr may be BC,DE,HL,AF)
// ----------------------------------------------------------------------------
fn push_bc(cpu: CPUState, mem: &mut Vec<Byte>) -> CPUState {
    mem[(cpu.sp - 0) as usize] = cpu.reg[REG_B];
    mem[(cpu.sp - 1) as usize] = cpu.reg[REG_C];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 16,
        sp: cpu.sp - 2,
        ..cpu
    }
}

//   pop  rr          x1        12 (AF) rr=(SP)  SP=SP+2   (rr may be BC,DE,HL,AF)
// ----------------------------------------------------------------------------
const fn pop_bc(cpu: CPUState, mem: &[Byte]) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_C] = mem[(cpu.sp + 0) as usize];
    reg[REG_B] = mem[(cpu.sp + 1) as usize];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 12,
        sp: cpu.sp + 2,
        reg,
        ..cpu
    }
}

// GMB 8bit-Arithmetic/logical Commands
// ============================================================================
const fn impl_add(cpu: CPUState, arg: Byte) -> CPUState {
    // z0hc
    let mut reg = cpu.reg;
    let reg_a: Byte = cpu.reg[REG_A];

    let h: bool = ((reg_a & 0x0f) + (arg & 0x0f)) & 0x10 > 0;
    let (result, c) = reg_a.overflowing_add(arg);
    let flags: Byte = if result == 0 {FL_Z} else {0} 
                    | if h {FL_H} else {0}
                    | if c {FL_C} else {0};
    
    reg[REG_A] = result;
    reg[FLAGS] = flags;

    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 4,
        reg,
        ..cpu
    }
}
const fn impl_adc(cpu: CPUState, arg: Byte) -> CPUState {
    // z0hc
    if cpu.reg[FLAGS] & FL_C > 0 {
        let cpu_pre = impl_add(cpu, arg);
        let cpu_post = impl_add(cpu_pre, 0x01);
        // ignore Z from pre but keep it in post
        // keep H and C flags if they were set in either operation
        let flags: Byte = cpu_post.reg[FLAGS]
        | (cpu_pre.reg[FLAGS] & (FL_H | FL_C));

        let mut reg = cpu_post.reg;
        reg[FLAGS] = flags;

        CPUState {
            pc: cpu.pc + 1,
            tsc: cpu.tsc + 4,
            reg,
            ..cpu_post
        }
    } else {
        impl_add(cpu, arg)
    }
}
const fn impl_xor(cpu: CPUState, arg: Byte) -> CPUState {
    // z000
    let mut reg = cpu.reg;

    reg[REG_A] = reg[REG_A] ^ arg;
    reg[FLAGS] = if reg[REG_A] == 0 {
        FL_Z
    } else {
        0x00
    };

    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 4,
        reg,
        ..cpu
    }
}
const fn impl_inc_dec(cpu: CPUState, dst: usize, flag_n: Byte) -> CPUState {
    // z0h- for inc
    // z1h- for dec
    let mut reg = cpu.reg;
    let (h, (res, _c)) = if flag_n > 0 {
        (
            reg[dst] & 0x0F == 0x00,
            reg[dst].overflowing_sub(1)
        )
    } else {
        (
            reg[dst] & 0x0F == 0x0F,
            reg[dst].overflowing_add(1)
        )
    };

    let flags = reg[FLAGS] & FL_C // maintain the carry, we'll set the rest
    | if res == 0x00 {FL_Z} else {0}
    | flag_n
    | if h {FL_H} else {0};

    reg[dst] = res;
    reg[FLAGS] = flags;

    CPUState {pc: cpu.pc + 1, tsc: cpu.tsc + 4, reg, ..cpu}
}
const fn impl_inc16(cpu: CPUState, high: usize, low: usize) -> CPUState {
    let mut reg = cpu.reg;
    let operand: Word = combine(reg[high], reg[low]);
    let (res, _) = operand.overflowing_add(1);
    reg[high] = hi(res);
    reg[low]  = lo(res);
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        reg,
        ..cpu
    }
}
// todo: cp is just sub without storing the result in A
const fn impl_cp(cpu: CPUState, d8: Byte) -> CPUState {
    // z1hc
    let mut reg = cpu.reg;
    let (_, h) = (cpu.reg[REG_A] & 0x0F).overflowing_sub(d8 & 0x0F);
    let c = d8 > cpu.reg[REG_A];
    let z = d8 == cpu.reg[REG_A];
    reg[FLAGS] = if z {FL_Z} else {0}
    | FL_N
    | if h {FL_H} else {0}
    | if c {FL_C} else {0};
    CPUState { pc: cpu.pc + 1, tsc: cpu.tsc + 4, reg, ..cpu }
}

//   add  A,r         8x         4 z0hc A=A+r
// ----------------------------------------------------------------------------
const fn add_b(cpu: CPUState) -> CPUState { impl_add(cpu, cpu.reg[REG_B]) }
const fn add_c(cpu: CPUState) -> CPUState { impl_add(cpu, cpu.reg[REG_C]) }
const fn add_d(cpu: CPUState) -> CPUState { impl_add(cpu, cpu.reg[REG_D]) }
const fn add_e(cpu: CPUState) -> CPUState { impl_add(cpu, cpu.reg[REG_E]) }
const fn add_h(cpu: CPUState) -> CPUState { impl_add(cpu, cpu.reg[REG_H]) }
const fn add_l(cpu: CPUState) -> CPUState { impl_add(cpu, cpu.reg[REG_L]) }
const fn add_a(cpu: CPUState) -> CPUState { impl_add(cpu, cpu.reg[REG_A]) }

//   add  A,n         C6 nn      8 z0hc A=A+n
// ----------------------------------------------------------------------------
const fn add_d8(cpu: CPUState, d8: Byte) -> CPUState {
    CPUState{
        pc: cpu.pc + 2,
        tsc: cpu.tsc + 8,
        ..impl_add(cpu, d8)
    }
}

//   add  A,(HL)      86         8 z0hc A=A+(HL)
// ----------------------------------------------------------------------------
const fn add_HL(cpu: CPUState, mem: &[Byte]) -> CPUState {
    CPUState {
        tsc: cpu.tsc + 8, 
        ..impl_add(cpu, mem[cpu.HL()])
    }
}

//   adc  A,r         8x         4 z0hc A=A+r+cy
// ----------------------------------------------------------------------------
const fn adc_b(cpu: CPUState) -> CPUState { impl_adc(cpu, cpu.reg[REG_B]) }
const fn adc_c(cpu: CPUState) -> CPUState { impl_adc(cpu, cpu.reg[REG_C]) }
const fn adc_d(cpu: CPUState) -> CPUState { impl_adc(cpu, cpu.reg[REG_D]) }
const fn adc_e(cpu: CPUState) -> CPUState { impl_adc(cpu, cpu.reg[REG_E]) }
const fn adc_h(cpu: CPUState) -> CPUState { impl_adc(cpu, cpu.reg[REG_H]) }
const fn adc_l(cpu: CPUState) -> CPUState { impl_adc(cpu, cpu.reg[REG_L]) }
const fn adc_a(cpu: CPUState) -> CPUState { impl_adc(cpu, cpu.reg[REG_A]) }

//   adc  A,n         CE nn      8 z0hc A=A+n+cy
// ----------------------------------------------------------------------------
const fn adc_d8(cpu: CPUState, d8: Byte) -> CPUState { 
    CPUState{
        pc: cpu.pc + 2,
        tsc: cpu.tsc + 8,
        ..impl_adc(cpu, d8)
    } 
}

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
const fn xor_b(cpu: CPUState) -> CPUState { impl_xor(cpu, cpu.reg[REG_B]) }
const fn xor_c(cpu: CPUState) -> CPUState { impl_xor(cpu, cpu.reg[REG_C]) }
const fn xor_d(cpu: CPUState) -> CPUState { impl_xor(cpu, cpu.reg[REG_D]) }
const fn xor_e(cpu: CPUState) -> CPUState { impl_xor(cpu, cpu.reg[REG_E]) }
const fn xor_h(cpu: CPUState) -> CPUState { impl_xor(cpu, cpu.reg[REG_H]) }
const fn xor_l(cpu: CPUState) -> CPUState { impl_xor(cpu, cpu.reg[REG_L]) }
const fn xor_a(cpu: CPUState) -> CPUState { impl_xor(cpu, cpu.reg[REG_A]) }

//   xor  n           EE nn      8 z000
// ----------------------------------------------------------------------------
const fn xor_d8(cpu: CPUState, d8: Byte) -> CPUState {
    CPUState{
        pc: cpu.pc + 2,
        tsc: cpu.tsc + 8,
        ..impl_xor(cpu, d8)
    }
}

//   xor  (HL)        AE         8 z000
//   or   r           Bx         4 z000 A=A | r
//   or   n           F6 nn      8 z000 A=A | n
//   or   (HL)        B6         8 z000 A=A | (HL)

//   cp   r           Bx         4 z1hc compare A-r
// ----------------------------------------------------------------------------
const fn cp_b(cpu: CPUState) -> CPUState { impl_cp(cpu, cpu.reg[REG_B]) }
const fn cp_c(cpu: CPUState) -> CPUState { impl_cp(cpu, cpu.reg[REG_C]) }
const fn cp_d(cpu: CPUState) -> CPUState { impl_cp(cpu, cpu.reg[REG_D]) }
const fn cp_e(cpu: CPUState) -> CPUState { impl_cp(cpu, cpu.reg[REG_E]) }
const fn cp_h(cpu: CPUState) -> CPUState { impl_cp(cpu, cpu.reg[REG_H]) }
const fn cp_l(cpu: CPUState) -> CPUState { impl_cp(cpu, cpu.reg[REG_L]) }
const fn cp_a(cpu: CPUState) -> CPUState { impl_cp(cpu, cpu.reg[REG_A]) }

//   cp   n           FE nn      8 z1hc compare A-n
// ----------------------------------------------------------------------------
const fn cp_d8(cpu: CPUState, d8: Byte) -> CPUState {
    CPUState {
        pc: cpu.pc + 2,
        tsc: cpu.tsc + 8,
        ..impl_cp(cpu, d8)
    }
}

//   cp   (HL)        BE         8 z1hc compare A-(HL)
// ----------------------------------------------------------------------------
const fn cp_HL(cpu: CPUState, mem: &[Byte]) -> CPUState {
    CPUState {
        tsc: cpu.tsc + 8,
        ..impl_cp(cpu, mem[cpu.HL()])
    }
}

//   inc  r           xx         4 z0h- r=r+1
// ----------------------------------------------------------------------------
const fn inc_b(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_B, 0) }
const fn inc_c(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_C, 0) }
const fn inc_d(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_D, 0) }
const fn inc_e(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_E, 0) }
const fn inc_h(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_H, 0) }
const fn inc_l(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_L, 0) }
const fn inc_a(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_A, 0) }

//   inc  (HL)        34        12 z0h- (HL)=(HL)+1
//   dec  r           xx         4 z1h- r=r-1
// ----------------------------------------------------------------------------
const fn dec_b(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_B, FL_N) }
const fn dec_c(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_C, FL_N) }
const fn dec_d(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_D, FL_N) }
const fn dec_e(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_E, FL_N) }
const fn dec_h(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_H, FL_N) }
const fn dec_l(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_L, FL_N) }
const fn dec_a(cpu: CPUState) -> CPUState { impl_inc_dec(cpu, REG_A, FL_N) }

//   dec  (HL)        35        12 z1h- (HL)=(HL)-1
//   daa              27         4 z-0x decimal adjust akku
//   cpl              2F         4 -11- A = A xor FF

// GMB 16bit-Arithmetic/logical Commands
// ============================================================================
//   add  HL,rr     x9           8 -0hc HL = HL+rr     ;rr may be BC,DE,HL,SP

//   inc  rr        x3           8 ---- rr = rr+1      ;rr may be BC,DE,HL,SP
// ----------------------------------------------------------------------------
const fn inc_bc(cpu: CPUState) -> CPUState { impl_inc16(cpu, REG_B, REG_C) }
const fn inc_de(cpu: CPUState) -> CPUState { impl_inc16(cpu, REG_D, REG_E) }
const fn inc_hl(cpu: CPUState) -> CPUState { impl_inc16(cpu, REG_H, REG_L) }
const fn inc_sp(cpu: CPUState) -> CPUState {
    let (res, _) = cpu.sp.overflowing_add(1);
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        sp: res,
        ..cpu
    }
}

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
const fn impl_jr(cpu: CPUState, arg: SByte, do_it: bool) -> CPUState {
    let offset = if do_it {arg as Word} else {0};
    let time = if do_it {12} else {8};
    CPUState {
        pc: cpu.pc.wrapping_add(offset),
        tsc: cpu.tsc + time,
        ..cpu
    }
}

//   jp   nn        C3 nn nn    16 ---- jump to nn, PC=nn
// ----------------------------------------------------------------------------
const fn jp_d16(cpu: CPUState, low: Byte, high: Byte) -> CPUState {
    CPUState {
        pc: combine(high, low),
        tsc: cpu.tsc + 16,
        ..cpu
    }
}

//   jp   HL        E9           4 ---- jump to HL, PC=HL
//   jp   f,nn      xx nn nn 16;12 ---- conditional jump if nz,z,nc,c

//   jr   PC+dd     18 dd       12 ---- relative jump to nn (PC=PC+/-7bit)
// ----------------------------------------------------------------------------
const fn jr_r8(cpu: CPUState, r8: SByte) -> CPUState { impl_jr(cpu, r8, true) }

//   jr   f,PC+dd   xx dd     12;8 ---- conditional relative jump if nz,z,nc,c
// ----------------------------------------------------------------------------
const fn jr_nz_r8(cpu: CPUState, r8: SByte) -> CPUState { impl_jr(cpu, r8, cpu.reg[FLAGS] & FL_Z == 0) }
const fn jr_nc_r8(cpu: CPUState, r8: SByte) -> CPUState { impl_jr(cpu, r8, cpu.reg[FLAGS] & FL_C == 0) }
const fn jr_z_r8(cpu: CPUState, r8: SByte)  -> CPUState { impl_jr(cpu, r8, cpu.reg[FLAGS] & FL_Z != 0) }
const fn jr_c_r8(cpu: CPUState, r8: SByte)  -> CPUState { impl_jr(cpu, r8, cpu.reg[FLAGS] & FL_C != 0) }

//   call nn        CD nn nn    24 ---- call to nn, SP=SP-2, (SP)=PC, PC=nn
// ----------------------------------------------------------------------------
fn call_d16(cpu: CPUState, mem: &mut Vec<Byte>, low: Byte, high: Byte) -> CPUState {
    mem[(cpu.sp - 0) as usize] = hi(cpu.pc);
    mem[(cpu.sp - 1) as usize] = lo(cpu.pc);
    CPUState {
        tsc: cpu.tsc + 24,
        sp: cpu.sp - 2,
        pc: combine(high, low),
        ..cpu
    }
}

//   call f,nn      xx nn nn 24;12 ---- conditional call if nz,z,nc,c

//   ret            C9          16 ---- return, PC=(SP), SP=SP+2
// ----------------------------------------------------------------------------
const fn ret(cpu: CPUState, mem: &[Byte]) -> CPUState {
    CPUState {
        pc: combine(mem[(cpu.sp+1) as usize], mem[(cpu.sp) as usize]),
        tsc: cpu.tsc + 16,
        sp: cpu.sp + 2,
        ..cpu
    }
}

//   ret  f         xx        20;8 ---- conditional return if nz,z,nc,c
//   reti           D9          16 ---- return and enable interrupts (IME=1)
//   rst  n         xx          16 ---- call to 00,08,10,18,20,28,30,38

fn main() {
    env_logger::init();

    // window management
    // -----------------
    let mut buffer: Vec<u32> = vec![0; GB_SCREEN_WIDTH * GB_SCREEN_HEIGHT];
    let mut window = Window::new(
        "cerboy",
        GB_SCREEN_WIDTH * 4,
        GB_SCREEN_HEIGHT * 4,
        WindowOptions::default(),
    )
    .unwrap_or_else(|e| {
        panic!("{}",e)
    });
    window.limit_update_rate(Some(std::time::Duration::from_micros(16600)));

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

    // memory stuff
    // ------------
    let mut mem: Vec<Byte> = init_mem();

    // loop
    // ------------
    while window.is_open() && !window.is_key_down(Key::Escape) {
        let mut c: usize = 0;
        for i in buffer.iter_mut() {
            *i = rom[c] as u32;
            c += 1;
        }

        // We unwrap here as we want this code to exit if it fails. Real applications may want to handle this in a different way
        window
            .update_with_buffer(&buffer, GB_SCREEN_WIDTH, GB_SCREEN_HEIGHT)
            .unwrap();
    }
}

#[cfg(test)]
mod tests_cpu {
    use super::*;

    const INITIAL: CPUState = CPUState::new();

    #[test]
    fn test_impl_xor_r() {
        let result = impl_xor(INITIAL, 0x13);
        assert_eq!(result.pc, INITIAL.pc + 1, "incorrect program counter");
        assert_eq!(result.tsc, INITIAL.tsc + 4, "incorrect time stamp counter");
        assert_eq!(result.reg[REG_A], 0x12, "incorrect value in reg_a (expected 0x{:X} got 0x{:X})", 0x12, result.reg[REG_A]);
        assert_eq!(result.reg[FLAGS], 0x00, "incorrect flags (expected 0x{:X} got 0x{:X})", 0x00, result.reg[FLAGS]);
    }

    #[test]
    fn test_xor_a() {
        let result = xor_a(INITIAL);
        assert_eq!(result.reg[REG_A], 0x00);
        assert_eq!(result.reg[FLAGS], 0x80);
    }

    #[test]
    fn test_xor_bc() {
        let state = CPUState {
            reg: [0xCD, 0x11, 0, 0, 0, 0, 0x80, 0x01],
            ..INITIAL
        };
        assert_eq!(xor_b(state).reg[REG_A], 0xCC);
        assert_eq!(xor_c(state).reg[REG_A], 0x10);
    }

    #[test]
    fn test_xor_d8() {
        let result = xor_d8(INITIAL, 0xFF);
        assert_eq!(result.pc, INITIAL.pc + 2, "incorrect program counter");
        assert_eq!(result.tsc, INITIAL.tsc + 8, "incorrect time stamp counter");
        assert_eq!(result.reg[REG_A], 0xFE, "incorrect xor value in reg a");
    }
    
    #[test]
    fn test_ld_r_r() {
        assert_eq!(ld_b_a(INITIAL).reg[REG_B], 0x01);
        assert_eq!(ld_b_b(INITIAL).reg[REG_B], 0x00);
        assert_eq!(ld_b_c(INITIAL).reg[REG_B], 0x13);
        assert_eq!(ld_b_d(INITIAL).reg[REG_B], 0x00);
        assert_eq!(ld_b_e(INITIAL).reg[REG_B], 0xD8);
        assert_eq!(ld_b_h(INITIAL).reg[REG_B], 0x01);
        assert_eq!(ld_b_l(INITIAL).reg[REG_B], 0x4D);

        assert_eq!(ld_c_a(INITIAL).reg[REG_C], 0x01);
        assert_eq!(ld_c_b(INITIAL).reg[REG_C], 0x00);
        assert_eq!(ld_c_c(INITIAL).reg[REG_C], 0x13);
        assert_eq!(ld_c_d(INITIAL).reg[REG_C], 0x00);
        assert_eq!(ld_c_e(INITIAL).reg[REG_C], 0xD8);
        assert_eq!(ld_c_h(INITIAL).reg[REG_C], 0x01);
        assert_eq!(ld_c_l(INITIAL).reg[REG_C], 0x4D);

        assert_eq!(ld_d_a(INITIAL).reg[REG_D], 0x01);
        assert_eq!(ld_d_b(INITIAL).reg[REG_D], 0x00);
        assert_eq!(ld_d_c(INITIAL).reg[REG_D], 0x13);
        assert_eq!(ld_d_d(INITIAL).reg[REG_D], 0x00);
        assert_eq!(ld_d_e(INITIAL).reg[REG_D], 0xD8);
        assert_eq!(ld_d_h(INITIAL).reg[REG_D], 0x01);
        assert_eq!(ld_d_l(INITIAL).reg[REG_D], 0x4D);

        assert_eq!(ld_e_a(INITIAL).reg[REG_E], 0x01);
        assert_eq!(ld_e_b(INITIAL).reg[REG_E], 0x00);
        assert_eq!(ld_e_c(INITIAL).reg[REG_E], 0x13);
        assert_eq!(ld_e_d(INITIAL).reg[REG_E], 0x00);
        assert_eq!(ld_e_e(INITIAL).reg[REG_E], 0xD8);
        assert_eq!(ld_e_h(INITIAL).reg[REG_E], 0x01);
        assert_eq!(ld_e_l(INITIAL).reg[REG_E], 0x4D);

        assert_eq!(ld_h_a(INITIAL).reg[REG_H], 0x01);
        assert_eq!(ld_h_b(INITIAL).reg[REG_H], 0x00);
        assert_eq!(ld_h_c(INITIAL).reg[REG_H], 0x13);
        assert_eq!(ld_h_d(INITIAL).reg[REG_H], 0x00);
        assert_eq!(ld_h_e(INITIAL).reg[REG_H], 0xD8);
        assert_eq!(ld_h_h(INITIAL).reg[REG_H], 0x01);
        assert_eq!(ld_h_l(INITIAL).reg[REG_H], 0x4D);

        assert_eq!(ld_l_a(INITIAL).reg[REG_L], 0x01);
        assert_eq!(ld_l_b(INITIAL).reg[REG_L], 0x00);
        assert_eq!(ld_l_c(INITIAL).reg[REG_L], 0x13);
        assert_eq!(ld_l_d(INITIAL).reg[REG_L], 0x00);
        assert_eq!(ld_l_e(INITIAL).reg[REG_L], 0xD8);
        assert_eq!(ld_l_h(INITIAL).reg[REG_L], 0x01);
        assert_eq!(ld_l_l(INITIAL).reg[REG_L], 0x4D);

        assert_eq!(ld_a_a(INITIAL).reg[REG_A], 0x01);
        assert_eq!(ld_a_b(INITIAL).reg[REG_A], 0x00);
        assert_eq!(ld_a_c(INITIAL).reg[REG_A], 0x13);
        assert_eq!(ld_a_d(INITIAL).reg[REG_A], 0x00);
        assert_eq!(ld_a_e(INITIAL).reg[REG_A], 0xD8);
        assert_eq!(ld_a_h(INITIAL).reg[REG_A], 0x01);
        assert_eq!(ld_a_l(INITIAL).reg[REG_A], 0x4D);
    }

    #[test]
    fn test_ld_r_d8() {
        assert_eq!(ld_a_d8(INITIAL, 0xAF).reg[REG_B], 0xAF);
        assert_eq!(ld_b_d8(INITIAL, 0xAF).reg[REG_C], 0xAF);
        assert_eq!(ld_c_d8(INITIAL, 0xAF).reg[REG_D], 0xAF);
        assert_eq!(ld_d_d8(INITIAL, 0xAF).reg[REG_E], 0xAF);
        assert_eq!(ld_e_d8(INITIAL, 0xAF).reg[REG_H], 0xAF);
        assert_eq!(ld_h_d8(INITIAL, 0xAF).reg[REG_L], 0xAF);
        assert_eq!(ld_l_d8(INITIAL, 0xAF).reg[REG_A], 0xAF);
    }

    #[test]
    fn test_add() {
        // reg a inits to 0x01
        assert_eq!(impl_add(INITIAL, 0xFF).reg[REG_A], 0x00, "failed 0xff");
        assert_eq!(impl_add(INITIAL, 0xFF).reg[FLAGS], FL_Z | FL_H | FL_C, "failed 0xff flags");

        assert_eq!(impl_add(INITIAL, 0x0F).reg[REG_A], 0x10, "failed 0x0f");
        assert_eq!(impl_add(INITIAL, 0x0F).reg[FLAGS], FL_H, "failed 0x0f flags");

        assert_eq!(impl_add(INITIAL, 0x01).reg[REG_A], 0x02, "failed 0x01");
        assert_eq!(impl_add(INITIAL, 0x01).reg[FLAGS], 0x00, "failed 0x01 flags");
    }

    #[test]
    fn test_adc() {
        let cpu = CPUState {
            reg: [0xCD, 0x11, 0, 0, 0, 0, 0x00, 0x01],
            ..INITIAL
        };
        let cpu_c = CPUState {
            reg: [0xCD, 0x11, 0, 0, 0, 0, FL_C, 0x01],
            ..INITIAL
        };

        assert_eq!(impl_adc(cpu, 0xFE).reg[REG_A], 0xFF, "failed plain 0xFE");
        assert_eq!(impl_adc(cpu_c, 0xFE).reg[REG_A], 0x00);
        assert_eq!(impl_adc(cpu_c, 0xFE).reg[FLAGS], FL_Z | FL_H | FL_C, "failed carry 0xFE");

        assert_eq!(impl_adc(cpu, 0x0F).reg[REG_A], 0x10);
        assert_eq!(impl_adc(cpu, 0x0F).reg[FLAGS], FL_H, "failed plain 0x0F");

        assert_eq!(impl_adc(cpu_c, 0x0F).reg[REG_A], 0x11);
        assert_eq!(impl_adc(cpu_c, 0x0F).reg[FLAGS], FL_H, "failed carry 0x0F");

        assert_eq!(impl_adc(cpu, 0x01).reg[REG_A], 0x02, "failed plain 0x01");
        assert_eq!(impl_adc(cpu, 0x01).reg[FLAGS], 0, "failed plain 0x01");

        assert_eq!(impl_adc(cpu_c, 0x01).reg[REG_A], 0x03, "failed carry 0x01");
        assert_eq!(impl_adc(cpu_c, 0x01).reg[FLAGS], 0, "failed carry flags 0x01");
    }

    #[test]
    fn test_add_HL() {
        let mut mem = init_mem();
        let cpu = CPUState {
            reg: [0, 0, 0, 0, 0, 0x01, 0, 0x01],
            ..INITIAL
        };
        mem[cpu.HL()] = 0x0F;
        assert_eq!(add_HL(cpu, &mem).reg[REG_A], 0x10);
        assert_eq!(add_HL(cpu, &mem).reg[FLAGS], FL_H);
    }

    #[test]
    fn test_call_d16() {
        let mut mem = init_mem();
        let result = call_d16(INITIAL, &mut mem, 0x01, 0x02);
        assert_eq!(mem[(INITIAL.sp - 0) as usize], hi(INITIAL.pc), "failed high check");
        assert_eq!(mem[(INITIAL.sp - 1) as usize], lo(INITIAL.pc), "failed low check");
        assert_eq!(result.pc, 0x0201, "failed sp check")
    }

    #[test]
    fn test_inc_dec() {
        let cpu = CPUState{
            //    B     C     D     E     H     L     fl    A
            reg: [0x0F, 0xFF, 0x0E, 0x00, 0x02, 0x03, FL_C, 0x01],
            ..INITIAL 
        };
        
        assert_eq!(inc_b(cpu).reg[REG_B], 0x10);
        assert_eq!(inc_b(cpu).reg[FLAGS], FL_H | FL_C);
        assert_eq!(dec_b(cpu).reg[REG_B], 0x0E);
        assert_eq!(dec_b(cpu).reg[FLAGS], FL_N | FL_C);
        
        assert_eq!(inc_c(cpu).reg[REG_C], 0x00);
        assert_eq!(inc_c(cpu).reg[FLAGS], FL_Z | FL_H | FL_C);
        assert_eq!(dec_c(cpu).reg[REG_C], 0xFE);
        assert_eq!(dec_c(cpu).reg[FLAGS], FL_N | FL_C);
        
        assert_eq!(inc_d(cpu).reg[REG_D], 0x0F);
        assert_eq!(inc_d(cpu).reg[FLAGS], FL_C);
        assert_eq!(dec_d(cpu).reg[REG_D], 0x0D);
        assert_eq!(dec_d(cpu).reg[FLAGS], FL_N | FL_C);
        
        assert_eq!(inc_e(cpu).reg[REG_E], 0x01);
        assert_eq!(inc_e(cpu).reg[FLAGS], FL_C);
        assert_eq!(dec_e(cpu).reg[REG_E], 0xFF);
        assert_eq!(dec_e(cpu).reg[FLAGS], FL_N | FL_H | FL_C);
        
        assert_eq!(inc_h(cpu).reg[REG_H], 0x03);
        assert_eq!(inc_h(cpu).reg[FLAGS], FL_C);
        assert_eq!(dec_h(cpu).reg[REG_H], 0x01);
        assert_eq!(dec_h(cpu).reg[FLAGS], FL_N | FL_C);
        
        assert_eq!(inc_l(cpu).reg[REG_L], 0x04);
        assert_eq!(inc_l(cpu).reg[FLAGS], FL_C);
        assert_eq!(dec_l(cpu).reg[REG_L], 0x02);
        assert_eq!(dec_l(cpu).reg[FLAGS], FL_N | FL_C);
        
        assert_eq!(inc_a(cpu).reg[REG_A], 0x02);
        assert_eq!(inc_a(cpu).reg[FLAGS], FL_C);
        assert_eq!(dec_a(cpu).reg[REG_A], 0x00);
        assert_eq!(dec_a(cpu).reg[FLAGS], FL_Z | FL_N | FL_C);
    }
    
    #[test]
    fn test_cp() {
        let cpu = CPUState{
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0x12, FL_C, 0x11],
            ..INITIAL 
        };
        let mut mem = init_mem();
        mem[cpu.HL()] = cpu.reg[REG_L];

        assert_eq!(cp_b(cpu).reg[FLAGS], FL_N);
        assert_eq!(cp_c(cpu).reg[FLAGS], FL_N);
        assert_eq!(cp_d(cpu).reg[FLAGS], FL_N|FL_H);
        assert_eq!(cp_e(cpu).reg[FLAGS], FL_N|FL_H);
        assert_eq!(cp_h(cpu).reg[FLAGS], FL_Z|FL_N);
        assert_eq!(cp_l(cpu).reg[FLAGS], FL_N|FL_H|FL_C);
        assert_eq!(cp_a(cpu).reg[FLAGS], FL_Z|FL_N);
        assert_eq!(cp_d8(cpu,0x12).reg[FLAGS], FL_N|FL_H|FL_C);
        assert_eq!(cp_HL(cpu, &mem).reg[FLAGS], FL_N|FL_H|FL_C);
    }

    #[test]
    fn test_inc16() {
        let cpu = CPUState{
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0xFF, FL_C, 0x11],
            sp: 0x00FF,
            ..INITIAL 
        };
        assert_eq!(inc_bc(cpu).reg[REG_B], 0x00);
        assert_eq!(inc_bc(cpu).reg[REG_C], 0x02);
        assert_eq!(inc_de(cpu).reg[REG_D], 0x02);
        assert_eq!(inc_de(cpu).reg[REG_E], 0x04);
        assert_eq!(inc_hl(cpu).reg[REG_H], 0x12);
        assert_eq!(inc_hl(cpu).reg[REG_L], 0x00);
        assert_eq!(inc_sp(cpu).sp, 0x0100);
    }

    #[test]
    fn test_jp() {
        let cpu_c = CPUState{
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0xFF, FL_C, 0x11],
            pc: 0xFF,
            ..INITIAL 
        };
        let cpu_z = CPUState{
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0xFF, FL_Z, 0x11],
            ..cpu_c
        };

        assert_eq!(jp_d16(cpu_c, 0x03, 0x02).pc, 0x0203);
        assert_eq!(jp_d16(cpu_c, 0x03, 0x02).tsc, 16);
        
        assert_eq!(jr_z_r8(cpu_z, 1).pc, 0x100);
        assert_eq!(jr_z_r8(cpu_z, -0xF).pc, 0xF0);
        assert_eq!(jr_z_r8(cpu_c, 1).pc, cpu_c.pc);
        
        assert_eq!(jr_nz_r8(cpu_c, 1).pc, cpu_c.pc + 1);
        assert_eq!(jr_nz_r8(cpu_z, 1).pc, cpu_z.pc);
        assert_eq!(jr_nz_r8(cpu_z, 1).tsc, cpu_z.tsc + 8);

        assert_eq!(jr_c_r8(cpu_c, 1).pc, cpu_c.pc + 1);
        assert_eq!(jr_c_r8(cpu_z, 1).pc, cpu_z.pc);
        assert_eq!(jr_c_r8(cpu_c, 1).tsc, cpu_c.tsc + 12);
        assert_eq!(jr_c_r8(cpu_z, 1).tsc, cpu_z.tsc + 8);

        assert_eq!(jr_nc_r8(cpu_c, 1).pc, cpu_c.pc);
        assert_eq!(jr_nc_r8(cpu_z, 1).pc, cpu_z.pc + 1);
        assert_eq!(jr_nc_r8(cpu_c, 1).tsc, cpu_c.tsc + 8);
        assert_eq!(jr_nc_r8(cpu_z, 1).tsc, cpu_z.tsc + 12);
    }
}
