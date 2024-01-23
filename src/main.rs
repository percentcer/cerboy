#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::identity_op)]
#![feature(const_trait_impl)]

extern crate minifb;
use minifb::{Key, Window, WindowOptions};

extern crate env_logger;

use std::ops::{Index, IndexMut};

use cerboy::bits::*;
use cerboy::io::init_rom;
use cerboy::types::*;
use cerboy::decode::decodeCB;

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
const MEM_BANK_00: Word = 0x0000;
// 4000-7FFF   16KB ROM Bank 01..NN (in cartridge, switchable bank number)
const MEM_BANK_NN: Word = 0x4000;
// 8000-9FFF   8KB Video RAM (VRAM) (switchable bank 0-1 in CGB Mode)
const MEM_VRAM: Word = 0x8000;
// A000-BFFF   8KB External RAM     (in cartridge, switchable bank, if any)
const MEM_EXT: Word = 0xA000;
// C000-CFFF   4KB Work RAM Bank 0 (WRAM)
const MEM_WRAM_0: Word = 0xC000;
// D000-DFFF   4KB Work RAM Bank 1 (WRAM)  (switchable bank 1-7 in CGB Mode)
const MEM_WRAM_1: Word = 0xD000;
// E000-FDFF   Same as C000-DDFF (ECHO)    (typically not used)
const MEM_ECHO: Word = 0xE000;
// FE00-FE9F   Sprite Attribute Table (OAM)
const MEM_OAM: Word = 0xFE00;
// FEA0-FEFF   Not Usable
const MEM_NOT_USABLE: Word = 0xFEA0;
// FF00-FF7F   I/O Ports
const MEM_IO_PORTS: Word = 0xFF00;
// FF80-FFFE   High RAM (HRAM)
const MEM_HRAM: Word = 0xFF80;
// FFFF        Interrupt Enable Register

const GB_SCREEN_WIDTH: usize = 160;
const GB_SCREEN_HEIGHT: usize = 144;
const ROM_MAX: usize = 0x200000;
const MEM_SIZE: usize = 0xFFFF + 1;

// classic gameboy only has four shades, white (00), light (01), dark (10), black (11)
const PAL_CLASSIC: [u32; 4] = [0xE0F8D0, 0x88C070, 0x346856, 0x081820];
const PAL_ICE_CREAM: [u32; 4] = [0xFFF6D3, 0xF9A875, 0xEB6B6F, 0x7C3F58];
const PAL_VBOY: [u32; 4] = [0xEF0000, 0xA40000, 0x550000, 0x000000];

fn palette_lookup(color: Byte, plt: Byte, lut: &[u32; 4]) -> u32 {
    let idx = match color & 0b11 {
        0b00 => plt & 0b11,                     // white
        0b01 => (plt & (0b11 << 2)) >> 2,       // light
        0b10 => (plt & (0b11 << 4)) >> 4,       // dark
        0b11 => (plt & (0b11 << 6)) >> 6,       // black
        _ => panic!("unknown color {}", color), // debug
    };
    lut[idx as usize]
}

// https://gbdev.gg8.se/files/docs/mirrors/pandocs.html#lcdstatusregister
const TICKS_PER_OAM_SEARCH: u64 = 80;
const TICKS_PER_VRAM_IO: u64 = 168; // roughly
const TICKS_PER_HBLANK: u64 = 208; // roughly
const TICKS_PER_SCANLINE: u64 = TICKS_PER_OAM_SEARCH + TICKS_PER_VRAM_IO + TICKS_PER_HBLANK;
const TICKS_PER_VBLANK: u64 = TICKS_PER_SCANLINE * 10; // 144 on screen + 10 additional lines
const TICKS_PER_FRAME: u64 = (TICKS_PER_SCANLINE * GB_SCREEN_HEIGHT as u64) + TICKS_PER_VBLANK; // 70224 cycles

const TICKS_PER_DIV_INC: u64 = 256;

// tile constants
const BYTES_PER_TILE: u16 = 16;

// interrupt flags
const FL_INT_VBLANK: Byte = 1 << 0;
const FL_INT_STAT: Byte = 1 << 1;
const FL_INT_TIMER: Byte = 1 << 2;
const FL_INT_SERIAL: Byte = 1 << 3;
const FL_INT_JOYPAD: Byte = 1 << 4;

// RST locations (vectors)
const VEC_RST_00: Word = 0x0000;
const VEC_RST_08: Word = 0x0008;
const VEC_RST_10: Word = 0x0010;
const VEC_RST_18: Word = 0x0018;
const VEC_RST_20: Word = 0x0020;
const VEC_RST_28: Word = 0x0028;
const VEC_RST_30: Word = 0x0030;
const VEC_RST_38: Word = 0x0038;

// Interrupt locations (vectors)
const VEC_INT_VBLANK: Word = 0x0040;
const VEC_INT_STAT: Word = 0x0048;
const VEC_INT_TIMER: Word = 0x0050;
const VEC_INT_SERIAL: Word = 0x0058;
const VEC_INT_JOYPAD: Word = 0x0060;
// named I/O memory locations [FF00..FF7F]
const JOYP: Word = 0xFF00;
// timers
const DIV: Word = 0xFF04;
const TIMA: Word = 0xFF05;
const TMA: Word = 0xFF06;
const TAC: Word = 0xFF07;
// audio
const NR10: Word = 0xFF10;
const NR11: Word = 0xFF11;
const NR12: Word = 0xFF12;
const NR14: Word = 0xFF14;
const NR21: Word = 0xFF16;
const NR22: Word = 0xFF17;
const NR24: Word = 0xFF19;
const NR30: Word = 0xFF1A;
const NR31: Word = 0xFF1B;
const NR32: Word = 0xFF1C;
const NR33: Word = 0xFF1E;
const NR41: Word = 0xFF20;
const NR42: Word = 0xFF21;
const NR43: Word = 0xFF22;
const NR44: Word = 0xFF23;
const NR50: Word = 0xFF24;
const NR51: Word = 0xFF25;
const NR52: Word = 0xFF26;
// rendering
const LCDC: Word = 0xFF40;
const STAT: Word = 0xFF41;
const SCY: Word = 0xFF42;
const SCX: Word = 0xFF43;
const LY: Word = 0xFF44;
const LYC: Word = 0xFF45;
const DMA: Word = 0xFF46; // <-- OAM memory transfer
const BGP: Word = 0xFF47;
const OBP0: Word = 0xFF48;
const OBP1: Word = 0xFF49;
const WY: Word = 0xFF4A;
const WX: Word = 0xFF4B;
// interrupt registers
const IF: Word = 0xFF0F;
const IE: Word = 0xFFFF;

struct Memory([Byte; MEM_SIZE]);
impl Index<Word> for Memory {
    type Output = Byte;

    fn index(&self, index: Word) -> &Self::Output {
        &self.0[index as usize]
    }
}
impl IndexMut<Word> for Memory {
    fn index_mut(&mut self, index: Word) -> &mut Self::Output {
        match index {
            DMA => println!("write DMA"),
            LCDC => println!("write LCDC"),
            _ => (),
        }

        &mut self.0[index as usize]
    }
}

#[derive(Copy, Clone, Debug)]
struct CPUState {
    tsc: u64, // counting cycles since reset, not part of actual gb hardware but used for instruction timing
    reg: [Byte; 8],
    sp: Word,
    pc: Word,
    ime: bool, // true == interrupts enabled
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
            pc: 0x0000,
            ime: false,
        }
    }

    /// Commonly used for addresses
    ///
    /// Combines the H and L registers into a usize for mem indexing
    const fn HL(&self) -> Word {
        combine(self.reg[REG_H], self.reg[REG_L])
    }
    /// Combines the B and C registers for 16b operations
    const fn BC(&self) -> Word {
        combine(self.reg[REG_B], self.reg[REG_C])
    }
    /// Combines the D and E registers for 16b operations
    const fn DE(&self) -> Word {
        combine(self.reg[REG_D], self.reg[REG_E])
    }

    /// Advance the program counter
    ///
    /// Advance pc by some amount and return the new state
    const fn adv_pc(&self, c: Word) -> CPUState {
        CPUState {
            pc: self.pc + c,
            ..*self
        }
    }

    /// Add time to the time stamp counter (tsc)
    ///
    /// Adds some number of cycles to the tsc and return a new state
    const fn tick(&self, t: u64) -> CPUState {
        CPUState {
            tsc: self.tsc + t,
            ..*self
        }
    }
}

struct HardwareTimers {
    timer: u64,
    divider: u64,
}

impl HardwareTimers {
    const fn new() -> HardwareTimers {
        HardwareTimers {
            timer: 0,
            divider: 0,
        }
    }
}

fn update_clocks(state: HardwareTimers, mem: &mut Memory, cycles: u64) -> HardwareTimers {
    // todo: If a TMA write is executed on the same cycle as the content
    // of TMA is transferred to TIMA due to a timer overflow,
    // the old value is transferred to TIMA.
    // https://gbdev.io/pandocs/Timer_and_Divider_Registers.html#ff06---tma---timer-modulo-rw
    // note: this implies you should save this value before executing the instruction
    // todo:
    let mut result = HardwareTimers {
        timer: state.timer + cycles,
        divider: state.divider + cycles,
    };

    while result.divider >= TICKS_PER_DIV_INC {
        // todo: only run this if gb isn't in STOP
        result.divider -= TICKS_PER_DIV_INC;
        mem_inc(mem, DIV);
    }

    let tac_cpi = match tac_cycles_per_inc(mem) {
        Ok(result) => result,
        Err(error) => panic!("{}", error),
    };

    if tac_enabled(mem) {
        while result.timer >= tac_cpi {
            // todo: consider moving this to some specialized memory management unit
            result.timer -= tac_cpi;
            let (_result, overflow) = mem_inc(mem, TIMA);
            if overflow {
                tima_reset(mem);
                request_interrupt(mem, FL_INT_TIMER);
            }
        }
    }

    result
}

fn init_mem() -> Memory {
    let mut mem = Memory([0; MEM_SIZE]);
    mem[TIMA] = 0x00;
    mem[TMA] = 0x00;
    mem[TAC] = 0x00;
    mem[NR10] = 0x80;
    mem[NR11] = 0xBF;
    mem[NR12] = 0xF3;
    mem[NR14] = 0xBF;
    mem[NR21] = 0x3F;
    mem[NR22] = 0x00;
    mem[NR24] = 0xBF;
    mem[NR30] = 0x7F;
    mem[NR31] = 0xFF;
    mem[NR32] = 0x9F;
    mem[NR33] = 0xBF;
    mem[NR41] = 0xFF;
    mem[NR42] = 0x00;
    mem[NR43] = 0x00;
    mem[NR44] = 0xBF;
    mem[NR50] = 0x77;
    mem[NR51] = 0xF3;
    mem[NR52] = 0xF1;
    mem[LCDC] = 0x91;
    mem[SCY] = 0x00;
    mem[SCX] = 0x00;
    mem[LYC] = 0x00;
    mem[BGP] = 0xFC;
    mem[OBP0] = 0xFF;
    mem[OBP1] = 0xFF;
    mem[WY] = 0x00;
    mem[WX] = 0x00;
    mem[IE] = 0x00;
    mem
}

// GMB 8bit-Loadcommands
// ============================================================================
const fn impl_ld_r_d8(cpu: CPUState, dst: usize, val: Byte) -> CPUState {
    let mut reg = cpu.reg;
    reg[dst] = val;
    CPUState { reg, ..cpu }
}
fn impl_ld_HL_d8(cpu: CPUState, mem: &mut Memory, val: Byte) -> CPUState {
    mem[cpu.HL()] = val;
    CPUState { ..cpu }
}

//   ld   r,r         xx         4 ---- r=r
// ----------------------------------------------------------------------------
// todo: the index arguments could be extracted from the opcode
const fn ld_b_b(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn ld_b_c(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn ld_b_d(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn ld_b_e(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn ld_b_h(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn ld_b_l(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn ld_b_a(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_B, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

const fn ld_c_b(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn ld_c_c(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn ld_c_d(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn ld_c_e(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn ld_c_h(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn ld_c_l(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn ld_c_a(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_C, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

const fn ld_d_b(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn ld_d_c(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn ld_d_d(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn ld_d_e(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn ld_d_h(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn ld_d_l(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn ld_d_a(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_D, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

const fn ld_e_b(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn ld_e_c(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn ld_e_d(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn ld_e_e(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn ld_e_h(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn ld_e_l(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn ld_e_a(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_E, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

const fn ld_h_b(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn ld_h_c(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn ld_h_d(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn ld_h_e(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn ld_h_h(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn ld_h_l(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn ld_h_a(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_H, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

const fn ld_l_b(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn ld_l_c(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn ld_l_d(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn ld_l_e(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn ld_l_h(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn ld_l_l(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn ld_l_a(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_L, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

const fn ld_a_b(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn ld_a_c(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn ld_a_d(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn ld_a_e(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn ld_a_h(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn ld_a_l(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn ld_a_a(cpu: CPUState) -> CPUState {
    impl_ld_r_d8(cpu, REG_A, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

//   ld   r,n         xx nn      8 ---- r=n
// ----------------------------------------------------------------------------
const fn ld_b_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_ld_r_d8(cpu, REG_B, d8).adv_pc(2).tick(8)
}
const fn ld_c_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_ld_r_d8(cpu, REG_C, d8).adv_pc(2).tick(8)
}
const fn ld_d_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_ld_r_d8(cpu, REG_D, d8).adv_pc(2).tick(8)
}
const fn ld_e_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_ld_r_d8(cpu, REG_E, d8).adv_pc(2).tick(8)
}
const fn ld_h_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_ld_r_d8(cpu, REG_H, d8).adv_pc(2).tick(8)
}
const fn ld_l_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_ld_r_d8(cpu, REG_L, d8).adv_pc(2).tick(8)
}
const fn ld_a_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_ld_r_d8(cpu, REG_A, d8).adv_pc(2).tick(8)
}

//   ld   r,(HL)      xx         8 ---- r=(HL)
// ----------------------------------------------------------------------------
fn ld_b_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ld_r_d8(cpu, REG_B, mem[cpu.HL()]).adv_pc(1).tick(8)
}
fn ld_c_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ld_r_d8(cpu, REG_C, mem[cpu.HL()]).adv_pc(1).tick(8)
}
fn ld_d_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ld_r_d8(cpu, REG_D, mem[cpu.HL()]).adv_pc(1).tick(8)
}
fn ld_e_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ld_r_d8(cpu, REG_E, mem[cpu.HL()]).adv_pc(1).tick(8)
}
fn ld_h_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ld_r_d8(cpu, REG_H, mem[cpu.HL()]).adv_pc(1).tick(8)
}
fn ld_l_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ld_r_d8(cpu, REG_L, mem[cpu.HL()]).adv_pc(1).tick(8)
}
fn ld_a_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ld_r_d8(cpu, REG_A, mem[cpu.HL()]).adv_pc(1).tick(8)
}

//   ld   (HL),r      7x         8 ---- (HL)=r
// ----------------------------------------------------------------------------
fn ld_HL_b(cpu: CPUState, mem: &mut Memory) -> CPUState {
    impl_ld_HL_d8(cpu, mem, cpu.reg[REG_B]).adv_pc(1).tick(8)
}
fn ld_HL_c(cpu: CPUState, mem: &mut Memory) -> CPUState {
    impl_ld_HL_d8(cpu, mem, cpu.reg[REG_C]).adv_pc(1).tick(8)
}
fn ld_HL_d(cpu: CPUState, mem: &mut Memory) -> CPUState {
    impl_ld_HL_d8(cpu, mem, cpu.reg[REG_D]).adv_pc(1).tick(8)
}
fn ld_HL_e(cpu: CPUState, mem: &mut Memory) -> CPUState {
    impl_ld_HL_d8(cpu, mem, cpu.reg[REG_E]).adv_pc(1).tick(8)
}
fn ld_HL_h(cpu: CPUState, mem: &mut Memory) -> CPUState {
    impl_ld_HL_d8(cpu, mem, cpu.reg[REG_H]).adv_pc(1).tick(8)
}
fn ld_HL_l(cpu: CPUState, mem: &mut Memory) -> CPUState {
    impl_ld_HL_d8(cpu, mem, cpu.reg[REG_L]).adv_pc(1).tick(8)
}
fn ld_HL_a(cpu: CPUState, mem: &mut Memory) -> CPUState {
    impl_ld_HL_d8(cpu, mem, cpu.reg[REG_A]).adv_pc(1).tick(8)
}

//   ld   (HL),n      36 nn     12 ----
// ----------------------------------------------------------------------------
fn ld_HL_d8(cpu: CPUState, val: Byte, mem: &mut Memory) -> CPUState {
    impl_ld_HL_d8(cpu, mem, val).adv_pc(2).tick(12)
}

//   ld   A,(BC)      0A         8 ----
// ----------------------------------------------------------------------------
fn ld_a_BC(cpu: CPUState, mem: &Memory) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_A] = mem[combine(reg[REG_B], reg[REG_C])];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        reg,
        ..cpu
    }
}

//   ld   A,(DE)      1A         8 ----
// ----------------------------------------------------------------------------
fn ld_a_DE(cpu: CPUState, mem: &Memory) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_A] = mem[combine(reg[REG_D], reg[REG_E])];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        reg,
        ..cpu
    }
}

//   ld   A,(nn)      FA nn nn        16 ----
// ----------------------------------------------------------------------------
fn ld_a_A16(low: Byte, high: Byte, cpu: CPUState, mem: &Memory) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_A] = mem[combine(high, low)];
    CPUState {
        pc: cpu.pc + 3,
        tsc: cpu.tsc + 16,
        ..cpu
    }
}

//   ld   (BC),A      02         8 ----
// ----------------------------------------------------------------------------
fn ld_BC_a(cpu: CPUState, mem: &mut Memory) -> CPUState {
    let addr = combine(cpu.reg[REG_B], cpu.reg[REG_C]);
    mem[addr] = cpu.reg[REG_A];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        ..cpu
    }
}

//   ld   (DE),A      12         8 ----
// ----------------------------------------------------------------------------
fn ld_DE_a(cpu: CPUState, mem: &mut Memory) -> CPUState {
    let addr = combine(cpu.reg[REG_D], cpu.reg[REG_E]);
    mem[addr] = cpu.reg[REG_A];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        ..cpu
    }
}

//   ld   (nn),A      EA nn nn        16 ----
// ----------------------------------------------------------------------------
fn ld_A16_a(low: Byte, high: Byte, cpu: CPUState, mem: &mut Memory) -> CPUState {
    let addr = combine(high, low);
    mem[addr] = cpu.reg[REG_A];
    CPUState {
        pc: cpu.pc + 3,
        tsc: cpu.tsc + 16,
        ..cpu
    }
}

//   ld   A,(FF00+n)  F0 nn     12 ---- read from io-port n (memory FF00+n)
// ----------------------------------------------------------------------------
fn ld_a_FF00_A8(cpu: CPUState, mem: &Memory, off: Byte) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_A] = mem[MEM_IO_PORTS + off as Word];
    CPUState {
        pc: cpu.pc + 2,
        tsc: cpu.tsc + 12,
        reg,
        ..cpu
    }
}

//   ld   (FF00+n),A  E0 nn     12 ---- write to io-port n (memory FF00+n)
// ----------------------------------------------------------------------------
fn ld_FF00_A8_a(off: Byte, cpu: CPUState, mem: &mut Memory) -> CPUState {
    mem[MEM_IO_PORTS + off as Word] = cpu.reg[REG_A];
    CPUState {
        pc: cpu.pc + 2,
        tsc: cpu.tsc + 12,
        ..cpu
    }
}

//   ld   A,(FF00+C)  F2         8 ---- read from io-port C (memory FF00+C)
// ----------------------------------------------------------------------------
fn ld_a_FF00_C(cpu: CPUState, mem: &Memory) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_A] = mem[MEM_IO_PORTS + reg[REG_C] as Word];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        reg,
        ..cpu
    }
}

//   ld   (FF00+C),A  E2         8 ---- write to io-port C (memory FF00+C)
// ----------------------------------------------------------------------------
fn ld_FF00_C_a(cpu: CPUState, mem: &mut Memory) -> CPUState {
    mem[MEM_IO_PORTS + cpu.reg[REG_C] as Word] = cpu.reg[REG_A];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        ..cpu
    }
}

//   ldi  (HL),A      22         8 ---- (HL)=A, HL=HL+1
// ----------------------------------------------------------------------------
fn ldi_HL_a(cpu: CPUState, mem: &mut Memory) -> CPUState {
    let mut reg = cpu.reg;
    let (hli, _) = combine(reg[REG_H], reg[REG_L]).overflowing_add(1);
    mem[cpu.HL()] = reg[REG_A];
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
// ----------------------------------------------------------------------------
fn ldi_a_HL(cpu: CPUState, mem: &mut Memory) -> CPUState {
    let mut reg = cpu.reg;
    let (hli, _) = combine(reg[REG_H], reg[REG_L]).overflowing_add(1);
    reg[REG_A] = mem[cpu.HL()];
    reg[REG_H] = hi(hli);
    reg[REG_L] = lo(hli);
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        reg,
        ..cpu
    }
}

//   ldd  (HL),A      32         8 ---- (HL)=A, HL=HL-1
// ----------------------------------------------------------------------------
fn ldd_HL_a(cpu: CPUState, mem: &mut Memory) -> CPUState {
    let mut reg = cpu.reg;
    let (hld, _) = combine(reg[REG_H], reg[REG_L]).overflowing_sub(1);
    mem[cpu.HL()] = reg[REG_A];
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
// ----------------------------------------------------------------------------
fn ldd_a_HL(cpu: CPUState, mem: &mut Memory) -> CPUState {
    let mut reg = cpu.reg;
    let (hld, _) = combine(reg[REG_H], reg[REG_L]).overflowing_sub(1);
    reg[REG_A] = mem[cpu.HL()];
    reg[REG_H] = hi(hld);
    reg[REG_L] = lo(hld);
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        reg,
        ..cpu
    }
}

// GMB 16bit-Loadcommands
// ============================================================================
const fn impl_ld_rr_d16(
    cpu: CPUState,
    reg_high: usize,
    reg_low: usize,
    high: Byte,
    low: Byte,
) -> CPUState {
    let mut reg = cpu.reg;
    reg[reg_high] = high;
    reg[reg_low] = low;
    CPUState { reg, ..cpu }
}

fn impl_push_rr(
    cpu: CPUState,
    mem: &mut Memory,
    reg_high: usize,
    reg_low: usize,
) -> CPUState {
    mem[cpu.sp - 1] = cpu.reg[reg_high];
    mem[cpu.sp - 2] = cpu.reg[reg_low];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 16,
        sp: cpu.sp - 2,
        ..cpu
    }
}

fn impl_pop_rr(
    cpu: CPUState,
    mem: &Memory,
    reg_high: usize,
    reg_low: usize,
) -> CPUState {
    let mut reg = cpu.reg;
    reg[reg_high] = mem[cpu.sp + 1];
    reg[reg_low] = mem[cpu.sp];
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 12,
        sp: cpu.sp + 2,
        reg,
        ..cpu
    }
}

//   ld   rr,nn       x1 nn nn  12 ---- rr=nn (rr may be BC,DE,HL or SP)
// ----------------------------------------------------------------------------
const fn ld_bc_d16(cpu: CPUState, low: Byte, high: Byte) -> CPUState {
    impl_ld_rr_d16(cpu, REG_B, REG_C, high, low)
        .adv_pc(3)
        .tick(12)
}
const fn ld_de_d16(cpu: CPUState, low: Byte, high: Byte) -> CPUState {
    impl_ld_rr_d16(cpu, REG_D, REG_E, high, low)
        .adv_pc(3)
        .tick(12)
}
const fn ld_hl_d16(cpu: CPUState, low: Byte, high: Byte) -> CPUState {
    impl_ld_rr_d16(cpu, REG_H, REG_L, high, low)
        .adv_pc(3)
        .tick(12)
}
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
fn push_bc(cpu: CPUState, mem: &mut Memory) -> CPUState { impl_push_rr(cpu, mem, REG_B, REG_C) }
fn push_de(cpu: CPUState, mem: &mut Memory) -> CPUState { impl_push_rr(cpu, mem, REG_D, REG_E) }
fn push_hl(cpu: CPUState, mem: &mut Memory) -> CPUState { impl_push_rr(cpu, mem, REG_H, REG_L) }
fn push_af(cpu: CPUState, mem: &mut Memory) -> CPUState { impl_push_rr(cpu, mem, REG_A, FLAGS) }

//   pop  rr          x1        12 (AF) rr=(SP)  SP=SP+2   (rr may be BC,DE,HL,AF)
// ----------------------------------------------------------------------------
fn pop_bc(cpu: CPUState, mem: &Memory) -> CPUState { impl_pop_rr(cpu, mem, REG_B, REG_C) }
fn pop_de(cpu: CPUState, mem: &Memory) -> CPUState { impl_pop_rr(cpu, mem, REG_D, REG_E) }
fn pop_hl(cpu: CPUState, mem: &Memory) -> CPUState { impl_pop_rr(cpu, mem, REG_H, REG_L) }
fn pop_af(cpu: CPUState, mem: &Memory) -> CPUState { impl_pop_rr(cpu, mem, REG_A, FLAGS) } // note that this one writes to flags

// GMB 8bit-Arithmetic/logical Commands
// ============================================================================
const fn impl_add(cpu: CPUState, arg: Byte) -> CPUState {
    // z0hc
    let mut reg = cpu.reg;
    let reg_a: Byte = cpu.reg[REG_A];

    let h: bool = ((reg_a & 0x0f) + (arg & 0x0f)) & 0x10 > 0;
    let (result, c) = reg_a.overflowing_add(arg);
    let flags: Byte =
        if result == 0 { FL_Z } else { 0 } | if h { FL_H } else { 0 } | if c { FL_C } else { 0 };
    reg[REG_A] = result;
    reg[FLAGS] = flags;

    CPUState { reg, ..cpu }
}
const fn impl_adc(cpu: CPUState, arg: Byte) -> CPUState {
    // z0hc
    if cpu.reg[FLAGS] & FL_C > 0 {
        let cpu_pre = impl_add(cpu, arg);
        let cpu_post = impl_add(cpu_pre, 0x01);
        // ignore Z from pre but keep it in post
        // keep H and C flags if they were set in either operation
        let flags: Byte = cpu_post.reg[FLAGS] | (cpu_pre.reg[FLAGS] & (FL_H | FL_C));

        let mut reg = cpu_post.reg;
        reg[FLAGS] = flags;

        CPUState { reg, ..cpu_post }
    } else {
        impl_add(cpu, arg)
    }
}
const fn impl_and(cpu: CPUState, arg: Byte) -> CPUState {
    // z010
    let mut reg = cpu.reg;

    reg[REG_A] &= arg;
    reg[FLAGS] = if reg[REG_A] == 0 { FL_Z } else { 0x00 } | FL_H;

    CPUState { reg, ..cpu }
}
const fn impl_xor(cpu: CPUState, arg: Byte) -> CPUState {
    // z000
    let mut reg = cpu.reg;

    reg[REG_A] ^= arg;
    reg[FLAGS] = if reg[REG_A] == 0 { FL_Z } else { 0x00 };

    CPUState { reg, ..cpu }
}
const fn impl_or(cpu: CPUState, arg: Byte) -> CPUState {
    // z000
    let mut reg = cpu.reg;

    reg[REG_A] |= arg;
    reg[FLAGS] = if reg[REG_A] == 0 { FL_Z } else { 0x00 };

    CPUState { reg, ..cpu }
}
const fn impl_inc_dec(cpu: CPUState, dst: usize, flag_n: Byte) -> CPUState {
    // z0h- for inc
    // z1h- for dec
    let mut reg = cpu.reg;
    let (h, (res, _c)) = if flag_n > 0 {
        (reg[dst] & 0x0F == 0x00, reg[dst].overflowing_sub(1))
    } else {
        (reg[dst] & 0x0F == 0x0F, reg[dst].overflowing_add(1))
    };

    let flags = reg[FLAGS] & FL_C // maintain the carry, we'll set the rest
    | if res == 0x00 {FL_Z} else {0}
    | flag_n
    | if h {FL_H} else {0};

    reg[dst] = res;
    reg[FLAGS] = flags;

    CPUState { reg, ..cpu }
}
const fn impl_inc16(cpu: CPUState, high: usize, low: usize) -> CPUState {
    let mut reg = cpu.reg;
    let operand: Word = combine(reg[high], reg[low]);
    let (res, _) = operand.overflowing_add(1);
    reg[high] = hi(res);
    reg[low] = lo(res);
    CPUState { reg, ..cpu }
}
const fn impl_dec16(cpu: CPUState, high: usize, low: usize) -> CPUState {
    let mut reg = cpu.reg;
    let operand: Word = combine(reg[high], reg[low]);
    let (res, _) = operand.overflowing_sub(1);
    reg[high] = hi(res);
    reg[low] = lo(res);
    CPUState { reg, ..cpu }
}
const fn impl_cp(cpu: CPUState, arg: Byte) -> CPUState {
    let mut reg = cpu.reg;
    let flagged = impl_sub(cpu, arg);
    reg[FLAGS] = flagged.reg[FLAGS];
    CPUState { reg, ..flagged }
}
const fn impl_sub(cpu: CPUState, arg: Byte) -> CPUState {
    // z1hc
    let mut reg = cpu.reg;
    let (_, h) = (cpu.reg[REG_A] & 0x0F).overflowing_sub(arg & 0x0F);
    let (res, c) = cpu.reg[REG_A].overflowing_sub(arg);
    let z = arg == cpu.reg[REG_A];
    reg[REG_A] = res;
    reg[FLAGS] =
        if z { FL_Z } else { 0 } | FL_N | if h { FL_H } else { 0 } | if c { FL_C } else { 0 };
    CPUState { reg, ..cpu }
}

//   add  A,r         8x         4 z0hc A=A+r
// ----------------------------------------------------------------------------
const fn add_b(cpu: CPUState) -> CPUState {
    impl_add(cpu, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn add_c(cpu: CPUState) -> CPUState {
    impl_add(cpu, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn add_d(cpu: CPUState) -> CPUState {
    impl_add(cpu, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn add_e(cpu: CPUState) -> CPUState {
    impl_add(cpu, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn add_h(cpu: CPUState) -> CPUState {
    impl_add(cpu, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn add_l(cpu: CPUState) -> CPUState {
    impl_add(cpu, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn add_a(cpu: CPUState) -> CPUState {
    impl_add(cpu, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

//   add  A,n         C6 nn      8 z0hc A=A+n
// ----------------------------------------------------------------------------
const fn add_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_add(cpu, d8).adv_pc(2).tick(8)
}

//   add  A,(HL)      86         8 z0hc A=A+(HL)
// ----------------------------------------------------------------------------
fn add_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_add(cpu, mem[cpu.HL()]).adv_pc(1).tick(8)
}

//   adc  A,r         8x         4 z0hc A=A+r+cy
// ----------------------------------------------------------------------------
const fn adc_b(cpu: CPUState) -> CPUState {
    impl_adc(cpu, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn adc_c(cpu: CPUState) -> CPUState {
    impl_adc(cpu, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn adc_d(cpu: CPUState) -> CPUState {
    impl_adc(cpu, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn adc_e(cpu: CPUState) -> CPUState {
    impl_adc(cpu, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn adc_h(cpu: CPUState) -> CPUState {
    impl_adc(cpu, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn adc_l(cpu: CPUState) -> CPUState {
    impl_adc(cpu, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn adc_a(cpu: CPUState) -> CPUState {
    impl_adc(cpu, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

//   adc  A,n         CE nn      8 z0hc A=A+n+cy
// ----------------------------------------------------------------------------
const fn adc_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_adc(cpu, d8).adv_pc(2).tick(8)
}

//   adc  A,(HL)      8E         8 z0hc A=A+(HL)+cy
// ----------------------------------------------------------------------------
fn adc_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_adc(cpu, mem[cpu.HL()]).adv_pc(1).tick(8)
}

//   sub  r           9x         4 z1hc A=A-r
// ----------------------------------------------------------------------------
const fn sub_b(cpu: CPUState) -> CPUState {
    impl_sub(cpu, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn sub_c(cpu: CPUState) -> CPUState {
    impl_sub(cpu, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn sub_d(cpu: CPUState) -> CPUState {
    impl_sub(cpu, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn sub_e(cpu: CPUState) -> CPUState {
    impl_sub(cpu, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn sub_h(cpu: CPUState) -> CPUState {
    impl_sub(cpu, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn sub_l(cpu: CPUState) -> CPUState {
    impl_sub(cpu, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn sub_a(cpu: CPUState) -> CPUState {
    impl_sub(cpu, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

//   sub  n           D6 nn      8 z1hc A=A-n
// ----------------------------------------------------------------------------
const fn sub_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_sub(cpu, d8).adv_pc(2).tick(8)
}

//   sub  (HL)        96         8 z1hc A=A-(HL)
// ----------------------------------------------------------------------------
fn sub_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_sub(cpu, mem[cpu.HL()]).adv_pc(1).tick(8)
}

//   sbc  A,r         9x         4 z1hc A=A-r-cy
//   sbc  A,n         DE nn      8 z1hc A=A-n-cy
//   sbc  A,(HL)      9E         8 z1hc A=A-(HL)-cy

//   and  r           Ax         4 z010 A=A & r
// ----------------------------------------------------------------------------
const fn and_b(cpu: CPUState) -> CPUState {
    impl_and(cpu, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn and_c(cpu: CPUState) -> CPUState {
    impl_and(cpu, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn and_d(cpu: CPUState) -> CPUState {
    impl_and(cpu, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn and_e(cpu: CPUState) -> CPUState {
    impl_and(cpu, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn and_h(cpu: CPUState) -> CPUState {
    impl_and(cpu, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn and_l(cpu: CPUState) -> CPUState {
    impl_and(cpu, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn and_a(cpu: CPUState) -> CPUState {
    impl_and(cpu, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

//   and  n           E6 nn      8 z010 A=A & n
// ----------------------------------------------------------------------------
const fn and_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_and(cpu, d8).adv_pc(2).tick(8)
}

//   and  (HL)        A6         8 z010 A=A & (HL)
// ----------------------------------------------------------------------------
fn and_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_and(cpu, mem[cpu.HL()]).adv_pc(1).tick(8)
}

//   xor  r           Ax         4 z000
// ----------------------------------------------------------------------------
const fn xor_b(cpu: CPUState) -> CPUState {
    impl_xor(cpu, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn xor_c(cpu: CPUState) -> CPUState {
    impl_xor(cpu, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn xor_d(cpu: CPUState) -> CPUState {
    impl_xor(cpu, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn xor_e(cpu: CPUState) -> CPUState {
    impl_xor(cpu, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn xor_h(cpu: CPUState) -> CPUState {
    impl_xor(cpu, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn xor_l(cpu: CPUState) -> CPUState {
    impl_xor(cpu, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn xor_a(cpu: CPUState) -> CPUState {
    impl_xor(cpu, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

//   xor  n           EE nn      8 z000
// ----------------------------------------------------------------------------
const fn xor_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_xor(cpu, d8).adv_pc(2).tick(8)
}

//   xor  (HL)        AE         8 z000
// ----------------------------------------------------------------------------
fn xor_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_xor(cpu, mem[cpu.HL()]).adv_pc(1).tick(8)
}

//   or   r           Bx         4 z000 A=A | r
// ----------------------------------------------------------------------------
const fn or_b(cpu: CPUState) -> CPUState {
    impl_or(cpu, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn or_c(cpu: CPUState) -> CPUState {
    impl_or(cpu, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn or_d(cpu: CPUState) -> CPUState {
    impl_or(cpu, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn or_e(cpu: CPUState) -> CPUState {
    impl_or(cpu, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn or_h(cpu: CPUState) -> CPUState {
    impl_or(cpu, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn or_l(cpu: CPUState) -> CPUState {
    impl_or(cpu, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn or_a(cpu: CPUState) -> CPUState {
    impl_or(cpu, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

//   or   n           F6 nn      8 z000 A=A | n
// ----------------------------------------------------------------------------
const fn or_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_or(cpu, d8).adv_pc(2).tick(8)
}

//   or   (HL)        B6         8 z000 A=A | (HL)
// ----------------------------------------------------------------------------
fn or_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_or(cpu, mem[cpu.HL()]).adv_pc(1).tick(8)
}

//   cp   r           Bx         4 z1hc compare A-r
// ----------------------------------------------------------------------------
const fn cp_b(cpu: CPUState) -> CPUState {
    impl_cp(cpu, cpu.reg[REG_B]).adv_pc(1).tick(4)
}
const fn cp_c(cpu: CPUState) -> CPUState {
    impl_cp(cpu, cpu.reg[REG_C]).adv_pc(1).tick(4)
}
const fn cp_d(cpu: CPUState) -> CPUState {
    impl_cp(cpu, cpu.reg[REG_D]).adv_pc(1).tick(4)
}
const fn cp_e(cpu: CPUState) -> CPUState {
    impl_cp(cpu, cpu.reg[REG_E]).adv_pc(1).tick(4)
}
const fn cp_h(cpu: CPUState) -> CPUState {
    impl_cp(cpu, cpu.reg[REG_H]).adv_pc(1).tick(4)
}
const fn cp_l(cpu: CPUState) -> CPUState {
    impl_cp(cpu, cpu.reg[REG_L]).adv_pc(1).tick(4)
}
const fn cp_a(cpu: CPUState) -> CPUState {
    impl_cp(cpu, cpu.reg[REG_A]).adv_pc(1).tick(4)
}

//   cp   n           FE nn      8 z1hc compare A-n
// ----------------------------------------------------------------------------
const fn cp_d8(cpu: CPUState, d8: Byte) -> CPUState {
    impl_cp(cpu, d8).adv_pc(2).tick(8)
}

//   cp   (HL)        BE         8 z1hc compare A-(HL)
// ----------------------------------------------------------------------------
fn cp_HL(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_cp(cpu, mem[cpu.HL()]).adv_pc(1).tick(8)
}

//   inc  r           xx         4 z0h- r=r+1
// ----------------------------------------------------------------------------
const fn inc_b(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_B, 0).adv_pc(1).tick(4)
}
const fn inc_c(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_C, 0).adv_pc(1).tick(4)
}
const fn inc_d(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_D, 0).adv_pc(1).tick(4)
}
const fn inc_e(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_E, 0).adv_pc(1).tick(4)
}
const fn inc_h(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_H, 0).adv_pc(1).tick(4)
}
const fn inc_l(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_L, 0).adv_pc(1).tick(4)
}
const fn inc_a(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_A, 0).adv_pc(1).tick(4)
}

//   inc  (HL)        34        12 z0h- (HL)=(HL)+1
// ----------------------------------------------------------------------------
fn inc_HL(cpu: CPUState, mem: &mut Memory) -> CPUState {
    let mut reg = cpu.reg;

    // z0h- for inc
    let (h, (res, _c)) = (mem[cpu.HL()] & 0x0F == 0x0F, mem[cpu.HL()].overflowing_add(1));
    
    let flags = reg[FLAGS] & FL_C // maintain the carry, we'll set the rest
    | if res == 0x00 {FL_Z} else {0}
    | if h {FL_H} else {0};
    reg[FLAGS] = flags;
    
    mem[cpu.HL()] = res;

    CPUState {
        reg,
        ..cpu
    }.adv_pc(1).tick(12)
}

//   dec  r           xx         4 z1h- r=r-1
// ----------------------------------------------------------------------------
const fn dec_b(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_B, FL_N).adv_pc(1).tick(4)
}
const fn dec_c(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_C, FL_N).adv_pc(1).tick(4)
}
const fn dec_d(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_D, FL_N).adv_pc(1).tick(4)
}
const fn dec_e(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_E, FL_N).adv_pc(1).tick(4)
}
const fn dec_h(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_H, FL_N).adv_pc(1).tick(4)
}
const fn dec_l(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_L, FL_N).adv_pc(1).tick(4)
}
const fn dec_a(cpu: CPUState) -> CPUState {
    impl_inc_dec(cpu, REG_A, FL_N).adv_pc(1).tick(4)
}

//   dec  (HL)        35        12 z1h- (HL)=(HL)-1
//   daa              27         4 z-0x decimal adjust akku

//   cpl              2F         4 -11- A = A xor FF
// ----------------------------------------------------------------------------
const fn cpl(cpu: CPUState) -> CPUState {
    let mut reg = cpu.reg;
    reg[REG_A] = reg[REG_A] ^ 0xFF;
    reg[FLAGS] = (reg[FLAGS] & FL_Z) | FL_N | FL_H | (reg[FLAGS] & FL_C);
    CPUState {
        reg,
        ..cpu
    }.adv_pc(1).tick(4)
}

// GMB 16bit-Arithmetic/logical Commands
// ============================================================================
//   add  HL,rr     x9           8 -0hc HL = HL+rr     ;rr may be BC,DE,HL,SP

//   inc  rr        x3           8 ---- rr = rr+1      ;rr may be BC,DE,HL,SP
// ----------------------------------------------------------------------------
const fn inc_bc(cpu: CPUState) -> CPUState {
    impl_inc16(cpu, REG_B, REG_C).adv_pc(1).tick(8)
}
const fn inc_de(cpu: CPUState) -> CPUState {
    impl_inc16(cpu, REG_D, REG_E).adv_pc(1).tick(8)
}
const fn inc_hl(cpu: CPUState) -> CPUState {
    impl_inc16(cpu, REG_H, REG_L).adv_pc(1).tick(8)
}
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
// ----------------------------------------------------------------------------
const fn dec_bc(cpu: CPUState) -> CPUState {
    impl_dec16(cpu, REG_B, REG_C).adv_pc(1).tick(8)
}
const fn dec_de(cpu: CPUState) -> CPUState {
    impl_dec16(cpu, REG_D, REG_E).adv_pc(1).tick(8)
}
const fn dec_hl(cpu: CPUState) -> CPUState {
    impl_dec16(cpu, REG_H, REG_L).adv_pc(1).tick(8)
}
const fn dec_sp(cpu: CPUState) -> CPUState {
    let (res, _) = cpu.sp.overflowing_sub(1);
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 8,
        sp: res,
        ..cpu
    }
}

//   add  SP,dd     E8          16 00hc SP = SP +/- dd ;dd is 8bit signed number
//   ld   HL,SP+dd  F8          12 00hc HL = SP +/- dd ;dd is 8bit signed number

// GMB Rotate- und Shift-Commands
// ============================================================================

//   rlca           07           4 000c rotate akku left
// ----------------------------------------------------------------------------
const fn rlca(cpu: CPUState) -> CPUState {
    let mut reg = cpu.reg;
    reg[FLAGS] = (cpu.reg[REG_A] & 0x80) >> 3;
    reg[REG_A] = cpu.reg[REG_A].rotate_left(1);
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 4,
        reg,
        ..cpu
    }
}

//   rla            17           4 000c rotate akku left through carry
// ----------------------------------------------------------------------------
const fn rla(cpu: CPUState) -> CPUState {
    let mut reg = cpu.reg;
    reg[FLAGS] = (cpu.reg[REG_A] & 0x80) >> 3;
    reg[REG_A] = (cpu.reg[REG_A].rotate_left(1) & 0xFE) | ((cpu.reg[FLAGS] & FL_C) >> 4);
    CPUState {
        pc: cpu.pc + 1,
        tsc: cpu.tsc + 4,
        reg,
        ..cpu
    }
}

//   rrca           0F           4 000c rotate akku right
//   rra            1F           4 000c rotate akku right through carry

//   rlc  r         CB 0x        8 z00c rotate left
// ----------------------------------------------------------------------------
const fn impl_rlc_r(cpu: CPUState, dst: usize) -> CPUState {
    let mut reg = cpu.reg;
    let result = reg[dst].rotate_left(1);
    let fl_c = if (result & 1) > 0 {FL_C} else {0};

    reg[dst] = result;
    reg[FLAGS] = fl_z(result) | fl_c;

    CPUState 
    {
        reg,
        ..cpu
    }.adv_pc(2).tick(8)
}

//   rlc  (HL)      CB 06       16 z00c rotate left
//   rl   r         CB 1x        8 z00c rotate left through carry
// ----------------------------------------------------------------------------
const fn impl_rl_r(cpu: CPUState, dst: usize) -> CPUState {
    let mut reg = cpu.reg;
    reg[dst] = (cpu.reg[dst].rotate_left(1) & 0xFE) | ((cpu.reg[FLAGS] & FL_C) >> 4);
    reg[FLAGS] = (cpu.reg[dst] & 0x80) >> 3 | if reg[dst] == 0 { FL_Z } else { 0 };
    CPUState { reg, ..cpu }.adv_pc(2).tick(8)
}

//   rl   (HL)      CB 16       16 z00c rotate left through carry
//   rrc  r         CB 0x        8 z00c rotate right
//   rrc  (HL)      CB 0E       16 z00c rotate right
//   rr   r         CB 1x        8 z00c rotate right through carry
//   rr   (HL)      CB 1E       16 z00c rotate right through carry
//   sla  r         CB 2x        8 z00c shift left arithmetic (b0=0)
//   sla  (HL)      CB 26       16 z00c shift left arithmetic (b0=0)
//   swap r         CB 3x        8 z000 exchange low/hi-nibble
// ----------------------------------------------------------------------------
const fn impl_swap_r(cpu: CPUState, dst: usize) -> CPUState {
    let mut reg = cpu.reg;
    reg[dst] = (reg[dst] >> 4) | (reg[dst] << 4);
    reg[FLAGS] = fl_z(reg[dst]);
    CPUState { reg, ..cpu }.adv_pc(2).tick(8)
}

//   swap (HL)      CB 36       16 z000 exchange low/hi-nibble
//   sra  r         CB 2x        8 z00c shift right arithmetic (b7=b7)
//   sra  (HL)      CB 2E       16 z00c shift right arithmetic (b7=b7)
//   srl  r         CB 3x        8 z00c shift right logical (b7=0)
//   srl  (HL)      CB 3E       16 z00c shift right logical (b7=0)

// GMB Singlebit Operation Commands
// ============================================================================
//   bit  n,r       CB xx        8 z01- test bit n
// ----------------------------------------------------------------------------
const fn impl_bit(cpu: CPUState, bit: Byte, dst: usize) -> CPUState {
    let mut reg = cpu.reg;
    let mask = 1 << bit;

    reg[FLAGS] = if (cpu.reg[dst] & mask) > 0 { FL_Z } else { 0 } | FL_H | (cpu.reg[FLAGS] & FL_C);
    CPUState { reg, ..cpu }.adv_pc(2).tick(8)
}

//   bit  n,(HL)    CB xx       12 z01- test bit n
//   set  n,r       CB xx        8 ---- set bit n
// ----------------------------------------------------------------------------
const fn impl_set(cpu: CPUState, bit: Byte, dst: usize) -> CPUState {
    let mut reg = cpu.reg;
    let mask = 1 << bit;

    reg[dst] |= mask;

    CPUState { reg, ..cpu }.adv_pc(2).tick(8)
}

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
    cpu.adv_pc(1).tick(4)
}

//   halt           76         N*4 ---- halt until interrupt occurs (low power)
//   stop           10 00        ? ---- low power standby mode (VERY low power)

//   di             F3           4 ---- disable interrupts, IME=0
// ----------------------------------------------------------------------------
const fn di(cpu: CPUState) -> CPUState {
    CPUState {
        ime: false,
        ..cpu.adv_pc(1).tick(4)
    }
}

//   ei             FB           4 ---- enable interrupts, IME=1
// ----------------------------------------------------------------------------
const fn ei(cpu: CPUState) -> CPUState {
    CPUState {
        ime: true,
        ..cpu.adv_pc(1).tick(4)
    }
}

// GMB Jumpcommands
// ============================================================================
const fn impl_jr(cpu: CPUState, arg: SByte) -> CPUState {
    CPUState {
        pc: cpu.pc.wrapping_add(arg as Word),
        ..cpu
    }
}

//   jp   nn        C3 nn nn    16 ---- jump to nn, PC=nn
// ----------------------------------------------------------------------------
const fn jp_d16(cpu: CPUState, low: Byte, high: Byte) -> CPUState {
    let cpu = cpu.adv_pc(3).tick(16);
    CPUState {
        pc: combine(high, low),
        ..cpu
    }
}

//   jp   HL        E9           4 ---- jump to HL, PC=HL
//   jp   f,nn      xx nn nn 16;12 ---- conditional jump if nz,z,nc,c

//   jr   PC+dd     18 dd       12 ---- relative jump to nn (PC=PC+/-7bit)
// ----------------------------------------------------------------------------
const fn jr_r8(cpu: CPUState, r8: SByte) -> CPUState {
    impl_jr(cpu.adv_pc(2), r8).tick(12)
}

//   jr   f,PC+dd   xx dd     12;8 ---- conditional relative jump if nz,z,nc,c
// ----------------------------------------------------------------------------
const fn jr_nz_r8(cpu: CPUState, r8: SByte) -> CPUState {
    let (time, offset) = if cpu.reg[FLAGS] & FL_Z == 0 {
        (12, r8)
    } else {
        (8, 0)
    };
    impl_jr(cpu.adv_pc(2), offset).tick(time)
}
const fn jr_nc_r8(cpu: CPUState, r8: SByte) -> CPUState {
    let (time, offset) = if cpu.reg[FLAGS] & FL_C == 0 {
        (12, r8)
    } else {
        (8, 0)
    };
    impl_jr(cpu.adv_pc(2), offset).tick(time)
}
const fn jr_z_r8(cpu: CPUState, r8: SByte) -> CPUState {
    let (time, offset) = if cpu.reg[FLAGS] & FL_Z != 0 {
        (12, r8)
    } else {
        (8, 0)
    };
    impl_jr(cpu.adv_pc(2), offset).tick(time)
}
const fn jr_c_r8(cpu: CPUState, r8: SByte) -> CPUState {
    let (time, offset) = if cpu.reg[FLAGS] & FL_C != 0 {
        (12, r8)
    } else {
        (8, 0)
    };
    impl_jr(cpu.adv_pc(2), offset).tick(time)
}

//   call nn        CD nn nn    24 ---- call to nn, SP=SP-2, (SP)=PC, PC=nn
// ----------------------------------------------------------------------------
fn call_d16(low: Byte, high: Byte, cpu: CPUState, mem: &mut Memory) -> CPUState {
    let cpu = cpu.adv_pc(3).tick(24);
    mem[cpu.sp - 0] = hi(cpu.pc);
    mem[cpu.sp - 1] = lo(cpu.pc);
    CPUState {
        sp: cpu.sp - 2,
        pc: combine(high, low),
        ..cpu
    }
}

//   call f,nn      xx nn nn 24;12 ---- conditional call if nz,z,nc,c

//   ret            C9          16 ---- return, PC=(SP), SP=SP+2
// ----------------------------------------------------------------------------
fn ret(cpu: CPUState, mem: &Memory) -> CPUState {
    CPUState {
        pc: combine(mem[cpu.sp + 2], mem[cpu.sp + 1]),
        tsc: cpu.tsc + 16,
        sp: cpu.sp + 2,
        ..cpu
    }
}

//   ret  f         xx        20;8 ---- conditional return if nz,z,nc,c
// ----------------------------------------------------------------------------
fn impl_ret_conditional(condition: bool, cpu: CPUState, mem: &Memory) -> CPUState {
    if condition {
        ret(cpu, mem).tick(4)
    } else {
        CPUState {
            pc: cpu.pc + 1,
            tsc: cpu.tsc + 8,
            ..cpu
        }
    }
}
fn ret_nz(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ret_conditional(cpu.reg[FLAGS] & FL_Z == 0, cpu, mem)
}
fn ret_z(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ret_conditional(cpu.reg[FLAGS] & FL_Z != 0, cpu, mem)
}
fn ret_nc(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ret_conditional(cpu.reg[FLAGS] & FL_C == 0, cpu, mem)
}
fn ret_c(cpu: CPUState, mem: &Memory) -> CPUState {
    impl_ret_conditional(cpu.reg[FLAGS] & FL_C != 0, cpu, mem)
}

//   reti           D9          16 ---- return and enable interrupts (IME=1)
// ----------------------------------------------------------------------------
fn reti(cpu: CPUState, mem: &Memory) -> CPUState {
    CPUState {
        ime: true,
        // except for the ime change, reti is identical to ret
        ..ret(cpu, mem)
    }
}

//   rst  n         xx          16 ---- call to 00,08,10,18,20,28,30,38
// ----------------------------------------------------------------------------
fn rst_n(cpu: CPUState, mem: &mut Memory, opcode: Byte) -> CPUState {
    let cpu = cpu.adv_pc(1).tick(16);

    let vst_hi = (opcode & HIGH_MASK_NIB) - 0xC0;
    let vst_lo = opcode & 0x08;
    let vst_addr = vst_hi | vst_lo;

    mem[cpu.sp - 0] = hi(cpu.pc);
    mem[cpu.sp - 1] = lo(cpu.pc);
    CPUState {
        sp: cpu.sp - 2,
        pc: vst_addr as Word,
        ..cpu
    }.tick(16)
}

// ============================================================================
// interrupts
// ============================================================================
fn handle_int(cpu: CPUState, mem: &mut Memory, fl_int: Byte, vec_int: Word) -> CPUState {
    mem[IF] &= !fl_int; // acknowledge the request flag (set to 0)
                        // push current position to stack to prepare for jump
    mem[cpu.sp - 0] = hi(cpu.pc);
    mem[cpu.sp - 1] = lo(cpu.pc);

    CPUState {
        ime: mem[IF] != 0, // only lock the ime if we're handling the final request
        // todo: acc: this behavior is incorrect, the ime should remain locked while handling the
        // SET OF interrupt requests that were enabled at the time of the handler invocation
        // e.g. if FL_INT_VSYNC and FL_INT_JOYPAD are requested then the interrupt handler
        // should execute both (in order of priority) but NOT execute any newly requested
        // interrupts until those are handled.
        sp: cpu.sp - 2,
        pc: vec_int,
        ..cpu.tick(20) // https://gbdev.io/pandocs/Interrupts.html#interrupt-handling
    }
}

// ============================================================================
// memory functions
// ============================================================================
fn request_interrupt(mem: &mut Memory, int_flag: Byte) {
    mem[IF] |= int_flag;
}

fn load_rom(mem: &mut Memory, rom: &[Byte]) {
    // raw copy, skip mem checks
    mem.0[0..rom.len()].copy_from_slice(rom)
}

fn mem_inc(mem: &mut Memory, loc: Word) -> (Byte, bool) {
    let (result, overflow) = mem[loc].overflowing_add(1);
    mem[loc] = result;
    (result, overflow)
}

fn tima_reset(mem: &mut Memory) {
    mem[TIMA] = mem[TMA];
}

fn tac_enabled(mem: &Memory) -> bool {
    mem[TAC] & 0b100 > 0
}

fn tac_cycles_per_inc(mem: &Memory) -> Result<u64, &'static str> {
    match mem[TAC] & 0b11 {
        0b00 => Ok(1024),
        0b01 => Ok(16),
        0b10 => Ok(64),
        0b11 => Ok(256),
        _ => Err("Invalid TAC clock setting"),
    }
}

fn lcd_mode(mem: &Memory) -> Byte {
    mem[STAT] & 0b11
}

fn set_lcd_mode(mode: Byte, mem: &mut Memory) {
    mem[STAT] = ((mem[STAT] >> 2) << 2) | (mode & 0b11);
}

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
    .unwrap_or_else(|e| panic!("{}", e));
    // todo: acc: changed timing here to make it more closely match the hardware
    // but I'm not sure why it's not running at the correct speed normally
    // (frame time should be longer, 16600)
    window.limit_update_rate(Some(std::time::Duration::from_micros(12600)));

    // arg processing
    // ---------
    let args: Vec<String> = std::env::args().collect();
    println!("{:?}", args);
    assert_eq!(
        args.len(),
        2,
        "unexpected number of args (must pass in path to rom)"
    );
    let rom_path: &str = &args[1];

    // init system
    // ------------
    let rom: Vec<Byte> = init_rom(rom_path);
    let mut cpu = CPUState::new();
    let mut mem: Memory = init_mem();
    load_rom(&mut mem, &rom); // load cartridge
    let boot = init_rom("./rom/boot/DMG_ROM.bin");
    load_rom(&mut mem, &boot);

    let mut timers = HardwareTimers::new();
    let mut lcd_timing: u64 = 0;

    // loop
    // ------------
    while window.is_open() && !window.is_key_down(Key::Escape) {
        // update
        // ------------------------------------------------

        // set start tsc for timer update (later)
        let tsc_prev = cpu.tsc;

        // check interrupts
        // -----------------
        // todo: The effect of EI is delayed by one instruction.
        // This means that EI followed immediately by DI does not
        // allow interrupts between the EI and the DI.
        if cpu.ime {
            let enabled_flags = mem[IE] & mem[IF];
            if (enabled_flags & FL_INT_VBLANK) > 0 {
                cpu = handle_int(cpu, &mut mem, FL_INT_VBLANK, VEC_INT_VBLANK);
            } else if (enabled_flags & FL_INT_STAT) > 0 {
                cpu = handle_int(cpu, &mut mem, FL_INT_STAT, VEC_INT_STAT);
            } else if (enabled_flags & FL_INT_TIMER) > 0 {
                cpu = handle_int(cpu, &mut mem, FL_INT_TIMER, VEC_INT_TIMER);
            } else if (enabled_flags & FL_INT_SERIAL) > 0 {
                cpu = handle_int(cpu, &mut mem, FL_INT_SERIAL, VEC_INT_SERIAL);
            } else if (enabled_flags & FL_INT_JOYPAD) > 0 {
                cpu = handle_int(cpu, &mut mem, FL_INT_JOYPAD, VEC_INT_JOYPAD);
            }
        }

        // fetch and execute
        // -----------------
        let pc = cpu.pc;
        cpu = match mem[pc] {
            0x00 => nop(cpu),
            0x01 => ld_bc_d16(cpu, mem[pc + 1], mem[pc + 2]),
            0x02 => ld_BC_a(cpu, &mut mem),
            0x03 => inc_bc(cpu),
            0x04 => inc_b(cpu),
            0x05 => dec_b(cpu),
            0x06 => ld_b_d8(cpu, mem[pc + 1]),
            0x07 => rlca(cpu),
            0x08 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x09 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x0A => ld_a_BC(cpu, &mem),
            0x0B => dec_bc(cpu),
            0x0C => inc_c(cpu),
            0x0D => dec_c(cpu),
            0x0E => ld_c_d8(cpu, mem[pc + 1]),
            0x0F => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x10 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x11 => ld_de_d16(cpu, mem[pc + 1], mem[pc + 2]),
            0x12 => ld_DE_a(cpu, &mut mem),
            0x13 => inc_de(cpu),
            0x14 => inc_d(cpu),
            0x15 => dec_d(cpu),
            0x16 => ld_d_d8(cpu, mem[pc + 1]),
            0x17 => rla(cpu),
            0x18 => jr_r8(cpu, signed(mem[pc + 1])),
            0x19 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x1A => ld_a_DE(cpu, &mem),
            0x1B => dec_de(cpu),
            0x1C => inc_e(cpu),
            0x1D => dec_e(cpu),
            0x1E => ld_e_d8(cpu, mem[pc + 1]),
            0x1F => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x20 => jr_nz_r8(cpu, signed(mem[pc + 1])),
            0x21 => ld_hl_d16(cpu, mem[pc + 1], mem[pc + 2]),
            0x22 => ldi_HL_a(cpu, &mut mem),
            0x23 => inc_hl(cpu),
            0x24 => inc_h(cpu),
            0x25 => dec_h(cpu),
            0x26 => ld_h_d8(cpu, mem[pc + 1]),
            0x27 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x28 => jr_z_r8(cpu, signed(mem[pc + 1])),
            0x29 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x2A => ldi_a_HL(cpu, &mut mem),
            0x2B => dec_hl(cpu),
            0x2C => inc_l(cpu),
            0x2D => dec_l(cpu),
            0x2E => ld_l_d8(cpu, mem[pc + 1]),
            0x2F => cpl(cpu),
            0x30 => jr_nc_r8(cpu, signed(mem[pc + 1])),
            0x31 => ld_sp_d16(cpu, mem[pc + 1], mem[pc + 2]),
            0x32 => ldd_HL_a(cpu, &mut mem),
            0x33 => inc_sp(cpu),
            0x34 => inc_HL(cpu, &mut mem),
            0x35 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x36 => ld_HL_d8(cpu, mem[pc + 1], &mut mem),
            0x37 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x38 => jr_c_r8(cpu, signed(mem[pc + 1])),
            0x39 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x3A => ldd_a_HL(cpu, &mut mem),
            0x3B => dec_sp(cpu),
            0x3C => inc_a(cpu),
            0x3D => dec_a(cpu),
            0x3E => ld_a_d8(cpu, mem[pc + 1]),
            0x3F => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x40 => ld_b_b(cpu),
            0x41 => ld_b_c(cpu),
            0x42 => ld_b_d(cpu),
            0x43 => ld_b_e(cpu),
            0x44 => ld_b_h(cpu),
            0x45 => ld_b_l(cpu),
            0x46 => ld_b_HL(cpu, &mem),
            0x47 => ld_b_a(cpu),
            0x48 => ld_c_b(cpu),
            0x49 => ld_c_c(cpu),
            0x4A => ld_c_d(cpu),
            0x4B => ld_c_e(cpu),
            0x4C => ld_c_h(cpu),
            0x4D => ld_c_l(cpu),
            0x4E => ld_c_HL(cpu, &mem),
            0x4F => ld_c_a(cpu),
            0x50 => ld_d_b(cpu),
            0x51 => ld_d_c(cpu),
            0x52 => ld_d_d(cpu),
            0x53 => ld_d_e(cpu),
            0x54 => ld_d_h(cpu),
            0x55 => ld_d_l(cpu),
            0x56 => ld_d_HL(cpu, &mem),
            0x57 => ld_d_a(cpu),
            0x58 => ld_e_b(cpu),
            0x59 => ld_e_c(cpu),
            0x5A => ld_e_d(cpu),
            0x5B => ld_e_e(cpu),
            0x5C => ld_e_h(cpu),
            0x5D => ld_e_l(cpu),
            0x5E => ld_e_HL(cpu, &mem),
            0x5F => ld_e_a(cpu),
            0x60 => ld_h_b(cpu),
            0x61 => ld_h_c(cpu),
            0x62 => ld_h_d(cpu),
            0x63 => ld_h_e(cpu),
            0x64 => ld_h_h(cpu),
            0x65 => ld_h_l(cpu),
            0x66 => ld_h_HL(cpu, &mem),
            0x67 => ld_h_a(cpu),
            0x68 => ld_l_b(cpu),
            0x69 => ld_l_c(cpu),
            0x6A => ld_l_d(cpu),
            0x6B => ld_l_e(cpu),
            0x6C => ld_l_h(cpu),
            0x6D => ld_l_l(cpu),
            0x6E => ld_l_HL(cpu, &mem),
            0x6F => ld_l_a(cpu),
            0x70 => ld_HL_b(cpu, &mut mem),
            0x71 => ld_HL_c(cpu, &mut mem),
            0x72 => ld_HL_d(cpu, &mut mem),
            0x73 => ld_HL_e(cpu, &mut mem),
            0x74 => ld_HL_h(cpu, &mut mem),
            0x75 => ld_HL_l(cpu, &mut mem),
            0x76 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x77 => ld_HL_a(cpu, &mut mem),
            0x78 => ld_a_b(cpu),
            0x79 => ld_a_c(cpu),
            0x7A => ld_a_d(cpu),
            0x7B => ld_a_e(cpu),
            0x7C => ld_a_h(cpu),
            0x7D => ld_a_l(cpu),
            0x7E => ld_a_HL(cpu, &mem),
            0x7F => ld_a_a(cpu),
            0x80 => add_b(cpu),
            0x81 => add_c(cpu),
            0x82 => add_d(cpu),
            0x83 => add_e(cpu),
            0x84 => add_h(cpu),
            0x85 => add_l(cpu),
            0x86 => add_HL(cpu, &mem),
            0x87 => add_a(cpu),
            0x88 => adc_b(cpu),
            0x89 => adc_c(cpu),
            0x8A => adc_d(cpu),
            0x8B => adc_e(cpu),
            0x8C => adc_h(cpu),
            0x8D => adc_l(cpu),
            0x8E => adc_HL(cpu, &mem),
            0x8F => adc_a(cpu),
            0x90 => sub_b(cpu),
            0x91 => sub_c(cpu),
            0x92 => sub_d(cpu),
            0x93 => sub_e(cpu),
            0x94 => sub_h(cpu),
            0x95 => sub_l(cpu),
            0x96 => sub_HL(cpu, &mem),
            0x97 => sub_a(cpu),
            0x98 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x99 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x9A => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x9B => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x9C => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x9D => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x9E => panic!("unknown instruction 0x{:X}", mem[pc]),
            0x9F => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xA0 => and_b(cpu),
            0xA1 => and_c(cpu),
            0xA2 => and_d(cpu),
            0xA3 => and_e(cpu),
            0xA4 => and_h(cpu),
            0xA5 => and_l(cpu),
            0xA6 => and_HL(cpu, &mem),
            0xA7 => and_a(cpu),
            0xA8 => xor_b(cpu),
            0xA9 => xor_c(cpu),
            0xAA => xor_d(cpu),
            0xAB => xor_e(cpu),
            0xAC => xor_h(cpu),
            0xAD => xor_l(cpu),
            0xAE => xor_HL(cpu, &mem),
            0xAF => xor_a(cpu),
            0xB0 => or_b(cpu),
            0xB1 => or_c(cpu),
            0xB2 => or_d(cpu),
            0xB3 => or_e(cpu),
            0xB4 => or_h(cpu),
            0xB5 => or_l(cpu),
            0xB6 => or_HL(cpu, &mem),
            0xB7 => or_a(cpu),
            0xB8 => cp_b(cpu),
            0xB9 => cp_c(cpu),
            0xBA => cp_d(cpu),
            0xBB => cp_e(cpu),
            0xBC => cp_h(cpu),
            0xBD => cp_l(cpu),
            0xBE => cp_HL(cpu, &mem),
            0xBF => cp_a(cpu),
            0xC0 => ret_nz(cpu, &mem),
            0xC1 => pop_bc(cpu, &mem),
            0xC2 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xC3 => jp_d16(cpu, mem[pc + 1], mem[pc + 2]),
            0xC4 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xC5 => push_bc(cpu, &mut mem),
            0xC6 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xC7 => rst_n(cpu, &mut mem, 0xC7),
            0xC8 => ret_z(cpu, &mem),
            0xC9 => ret(cpu, &mem),
            0xCA => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xCB => {
                let icb = decodeCB(mem[pc + 1]);
                match icb.opcode {
                    "RLC" => impl_rlc_r(cpu, icb.reg),
                    // "RRC" => panic!("unknown instruction (0xCB) 0x{:X}", mem[pc]),
                    "RL" => impl_rl_r(cpu, icb.reg),
                    // "RR" => panic!("unknown instruction (0xCB) 0x{:X}", mem[pc]),
                    // "SLA" => panic!("unknown instruction (0xCB) 0x{:X}", mem[pc]),
                    // "SRA" => panic!("unknown instruction (0xCB) 0x{:X}", mem[pc]),
                    "SWAP" => impl_swap_r(cpu, icb.reg),
                    // "SRL" => panic!("unknown instruction (0xCB) 0x{:X}", mem[pc]),
                    "BIT" => impl_bit(cpu, icb.bit, icb.reg),
                    // "RES" => panic!("unknown instruction (0xCB) 0x{:X}", mem[pc]),
                    "SET" => impl_set(cpu, icb.bit, icb.reg),
                    _ => panic!("unknown instruction (0xCB) 0x{:X}", mem[pc + 1])
                }
            },
            // match mem[pc + 1] {
            //     0x10 => rl_b(cpu),
            //     0x11 => rl_c(cpu),
            //     0x12 => rl_d(cpu),
            //     0x13 => rl_e(cpu),
            //     0x14 => rl_h(cpu),
            //     0x15 => rl_l(cpu),
            //     // 0x16 => rl_HL(cpu),
            //     0x17 => rl_a(cpu),
            //     0x7C => bit_7_h(cpu),
            //     _ => panic!("unknown instruction (0xCB) 0x{:X}", mem[pc]),
            // },
            0xCC => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xCD => call_d16(mem[pc + 1], mem[pc + 2], cpu, &mut mem),
            0xCE => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xCF => rst_n(cpu, &mut mem, 0xCF),
            0xD0 => ret_nc(cpu, &mem),
            0xD1 => pop_de(cpu, &mem),
            0xD2 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xD3 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xD4 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xD5 => push_de(cpu, &mut mem),
            0xD6 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xD7 => rst_n(cpu, &mut mem, 0xD7),
            0xD8 => ret_c(cpu, &mem),
            0xD9 => reti(cpu, &mem),
            0xDA => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xDB => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xDC => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xDD => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xDE => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xDF => rst_n(cpu, &mut mem, 0xDF),
            0xE0 => ld_FF00_A8_a(mem[pc + 1], cpu, &mut mem),
            0xE1 => pop_hl(cpu, &mem),
            0xE2 => ld_FF00_C_a(cpu, &mut mem),
            0xE3 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xE4 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xE5 => push_hl(cpu, &mut mem),
            0xE6 => and_d8(cpu, mem[pc + 1]),
            0xE7 => rst_n(cpu, &mut mem, 0xE7),
            0xE8 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xE9 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xEA => ld_A16_a(mem[pc + 1], mem[pc + 2], cpu, &mut mem),
            0xEB => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xEC => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xED => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xEE => xor_d8(cpu, mem[pc + 1]),
            0xEF => rst_n(cpu, &mut mem, 0xEF),
            0xF0 => ld_a_FF00_A8(cpu, &mem, mem[pc + 1]),
            0xF1 => pop_af(cpu, &mem),
            0xF2 => ld_a_FF00_C(cpu, &mem),
            0xF3 => di(cpu),
            0xF4 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xF5 => push_af(cpu, &mut mem),
            0xF6 => or_d8(cpu, mem[pc + 1]),
            0xF7 => rst_n(cpu, &mut mem, 0xF7),
            0xF8 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xF9 => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xFA => ld_a_A16(mem[pc + 1], mem[pc + 2], cpu, &mem),
            0xFB => ei(cpu),
            0xFC => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xFD => panic!("unknown instruction 0x{:X}", mem[pc]),
            0xFE => cp_d8(cpu, mem[pc + 1]),
            0xFF => rst_n(cpu, &mut mem, 0xFF),
        };
        let dt_cyc = cpu.tsc - tsc_prev;

        // update timers
        // -----------------
        timers = update_clocks(timers, &mut mem, dt_cyc);
        lcd_timing += dt_cyc;

        // render
        // ------------------------------------------------
        match lcd_mode(&mem) {
            // oam search
            2 => {
                if lcd_timing >= TICKS_PER_OAM_SEARCH {
                    set_lcd_mode(3, &mut mem);
                    lcd_timing -= TICKS_PER_OAM_SEARCH;
                }
            }
            // vram io
            3 => {
                if lcd_timing >= TICKS_PER_VRAM_IO {
                    // draw the scanline
                    // ===========================================
                    let ln_start: usize = GB_SCREEN_WIDTH * mem[LY] as usize;
                    let ln_end: usize = ln_start + GB_SCREEN_WIDTH;

                    // draw background
                    // -------------------------------------------
                    // todo: acc: this code is inaccurate, LCDC can actually be modified mid-scanline
                    // but cerboy currently only draws the line in a single shot (instead of per-dot)
                    let bg_tilemap_start: Word = if bit_test(3, mem[LCDC]) { 0x9C00 } else { 0x9800 };
                    let (bg_signed_addressing, bg_tile_data_start) = if bit_test(4, mem[LCDC]) {
                        (false, MEM_VRAM as Word)
                    } else {
                        // in signed addressing the 0 tile is at 0x9000
                        (true, MEM_VRAM + 0x1000 as Word)
                        // (true, MEM_VRAM + 0x0800 as Word) // <--- actual range starts at 0x8800 but that is -127, not zero
                    };
                    let (bg_y, _) = mem[SCY].overflowing_add(mem[LY]);
                    let bg_tile_line = bg_y as Word % 8;

                    // todo: removeme: for fun
                    // mem[SCX] = (f32::sin((mem[LY] as f32) * 0.1f32 + (cpu.tsc as f32)*0.000001f32)*5f32).trunc() as Byte;

                    for (c, i) in buffer[ln_start..ln_end].iter_mut().enumerate() {
                        let (bg_x, _) = mem[SCX].overflowing_add(c as Byte);
                        let bg_tile_index: Word = bg_x as Word / 8 + bg_y as Word / 8 * 32;
                        let bg_tile_id = mem[bg_tilemap_start + bg_tile_index];
                        let bg_tile_data_offset = if bg_signed_addressing {
                            (signed(bg_tile_id) as Word).wrapping_mul(BYTES_PER_TILE)
                        } else {
                            bg_tile_id as Word * BYTES_PER_TILE
                        };
                        let bg_tile_data = bg_tile_data_start.wrapping_add(bg_tile_data_offset);
                        let bg_tile_line_offset = bg_tile_data + bg_tile_line * 2;
                        let bg_tile_line_low_byte = mem[bg_tile_line_offset];
                        let bg_tile_line_high_byte = mem[bg_tile_line_offset + 1];
                        let bg_tile_current_pixel = 7 - ((c as Byte + mem[SCX]) % 8);
                        let bg_tile_pixel_mask = 1 << bg_tile_current_pixel;
                        let bg_tile_high_value = ((bg_tile_line_high_byte & bg_tile_pixel_mask)
                            >> bg_tile_current_pixel)
                            << 1;
                        let bg_tile_low_value =
                            (bg_tile_line_low_byte & bg_tile_pixel_mask) >> bg_tile_current_pixel;
                        let bg_tile_pixel_color_id = bg_tile_high_value | bg_tile_low_value;
                        *i = palette_lookup(bg_tile_pixel_color_id, mem[BGP], &PAL_VBOY);
                    }

                    // draw sprites
                    // FE00-FE9F   Sprite Attribute Table (OAM)
                    // -------------------------------------------
                    // for (c, i) in buffer[ln_start..ln_end].iter_mut().enumerate() {
                    // oijf
                    // }

                    // draw window
                    // -------------------------------------------
                    // for i in buffer[ln_start..ln_end].iter_mut() {}

                    // ===========================================

                    set_lcd_mode(0, &mut mem);
                    lcd_timing -= TICKS_PER_VRAM_IO;
                }
            }
            // hblank
            0 => {
                if lcd_timing >= TICKS_PER_HBLANK {
                    mem[LY] += 1;
                    lcd_timing -= TICKS_PER_HBLANK;
                    if mem[LY] == GB_SCREEN_HEIGHT as Byte {
                        // values 144 to 153 are vblank
                        request_interrupt(&mut mem, FL_INT_VBLANK);
                        set_lcd_mode(1, &mut mem);
                    } else {
                        set_lcd_mode(2, &mut mem);
                    }
                }
            }
            // vblank
            1 => {
                mem[LY] = (GB_SCREEN_HEIGHT as u64 + lcd_timing / TICKS_PER_SCANLINE) as Byte;
                if lcd_timing >= TICKS_PER_VBLANK {
                    mem[LY] = 0;
                    set_lcd_mode(2, &mut mem);
                    lcd_timing -= TICKS_PER_VBLANK;

                    window
                        .update_with_buffer(&buffer, GB_SCREEN_WIDTH, GB_SCREEN_HEIGHT)
                        .unwrap();

                    // print LCDC diagnostics
                    let lcdc_7 = if bit_test(7, mem[LCDC]) { " on" } else { "off" };
                    let lcdc_6 = if bit_test(6, mem[LCDC]) {
                        "0x9C00"
                    } else {
                        "0x9800"
                    };
                    let lcdc_5 = if bit_test(5, mem[LCDC]) { " on" } else { "off" };
                    let lcdc_4 = if bit_test(4, mem[LCDC]) {
                        "0x8000"
                    } else {
                        "0x8800"
                    };
                    let lcdc_3 = if bit_test(3, mem[LCDC]) {
                        "0x9C00"
                    } else {
                        "0x9800"
                    };
                    let lcdc_2 = if bit_test(2, mem[LCDC]) { "16" } else { " 8" };
                    let lcdc_1 = if bit_test(1, mem[LCDC]) { " on" } else { "off" };
                    let lcdc_0 = if bit_test(0, mem[LCDC]) { " on" } else { "off" };
                    let lcdc_v = mem[LCDC];
                    println!("{lcdc_v:#10b} LCDC [scr: {lcdc_7}, wnd_map: {lcdc_6}, wnd: {lcdc_5}, bg/wnd_dat: {lcdc_4}, bg_map: {lcdc_3}, obj_sz: {lcdc_2}, obj: {lcdc_1}, bg: {lcdc_0}]");
                }
            }
            _ => panic!("invalid LCD mode"),
        };
    }
}

#[cfg(test)]
mod tests_cpu {
    use super::*;

    const INITIAL: CPUState = CPUState::new();

    #[test]
    fn test_impl_xor_r() {
        let result = impl_xor(INITIAL, 0x13).adv_pc(1).tick(4);
        assert_eq!(result.pc, INITIAL.pc + 1, "incorrect program counter");
        assert_eq!(result.tsc, INITIAL.tsc + 4, "incorrect time stamp counter");
        assert_eq!(
            result.reg[REG_A], 0x12,
            "incorrect value in reg_a (expected 0x{:X} got 0x{:X})",
            0x12, result.reg[REG_A]
        );
        assert_eq!(
            result.reg[FLAGS], 0x00,
            "incorrect flags (expected 0x{:X} got 0x{:X})",
            0x00, result.reg[FLAGS]
        );
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
        assert_eq!(ld_b_d8(INITIAL, 0xAF).reg[REG_B], 0xAF);
        assert_eq!(ld_c_d8(INITIAL, 0xAF).reg[REG_C], 0xAF);
        assert_eq!(ld_d_d8(INITIAL, 0xAF).reg[REG_D], 0xAF);
        assert_eq!(ld_e_d8(INITIAL, 0xAF).reg[REG_E], 0xAF);
        assert_eq!(ld_h_d8(INITIAL, 0xAF).reg[REG_H], 0xAF);
        assert_eq!(ld_l_d8(INITIAL, 0xAF).reg[REG_L], 0xAF);
        assert_eq!(ld_a_d8(INITIAL, 0xAF).reg[REG_A], 0xAF);
    }

    #[test]
    fn test_ld_rr_d16() {
        assert_eq!(ld_bc_d16(INITIAL, 0xEF, 0xBE).reg[REG_B], 0xBE);
        assert_eq!(ld_bc_d16(INITIAL, 0xEF, 0xBE).reg[REG_C], 0xEF);
        assert_eq!(ld_de_d16(INITIAL, 0xAD, 0xDE).reg[REG_D], 0xDE);
        assert_eq!(ld_de_d16(INITIAL, 0xAD, 0xDE).reg[REG_E], 0xAD);
        assert_eq!(ld_hl_d16(INITIAL, 0xCE, 0xFA).reg[REG_H], 0xFA);
        assert_eq!(ld_hl_d16(INITIAL, 0xCE, 0xFA).reg[REG_L], 0xCE);
        assert_eq!(ld_sp_d16(INITIAL, 0xED, 0xFE).sp, 0xFEED);
    }

    #[test]
    fn test_add() {
        // reg a inits to 0x01
        assert_eq!(impl_add(INITIAL, 0xFF).reg[REG_A], 0x00, "failed 0xff");
        assert_eq!(
            impl_add(INITIAL, 0xFF).reg[FLAGS],
            FL_Z | FL_H | FL_C,
            "failed 0xff flags"
        );

        assert_eq!(impl_add(INITIAL, 0x0F).reg[REG_A], 0x10, "failed 0x0f");
        assert_eq!(
            impl_add(INITIAL, 0x0F).reg[FLAGS],
            FL_H,
            "failed 0x0f flags"
        );

        assert_eq!(impl_add(INITIAL, 0x01).reg[REG_A], 0x02, "failed 0x01");
        assert_eq!(
            impl_add(INITIAL, 0x01).reg[FLAGS],
            0x00,
            "failed 0x01 flags"
        );
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
        assert_eq!(
            impl_adc(cpu_c, 0xFE).reg[FLAGS],
            FL_Z | FL_H | FL_C,
            "failed carry 0xFE"
        );

        assert_eq!(impl_adc(cpu, 0x0F).reg[REG_A], 0x10);
        assert_eq!(impl_adc(cpu, 0x0F).reg[FLAGS], FL_H, "failed plain 0x0F");

        assert_eq!(impl_adc(cpu_c, 0x0F).reg[REG_A], 0x11);
        assert_eq!(impl_adc(cpu_c, 0x0F).reg[FLAGS], FL_H, "failed carry 0x0F");

        assert_eq!(impl_adc(cpu, 0x01).reg[REG_A], 0x02, "failed plain 0x01");
        assert_eq!(impl_adc(cpu, 0x01).reg[FLAGS], 0, "failed plain 0x01");

        assert_eq!(impl_adc(cpu_c, 0x01).reg[REG_A], 0x03, "failed carry 0x01");
        assert_eq!(
            impl_adc(cpu_c, 0x01).reg[FLAGS],
            0,
            "failed carry flags 0x01"
        );
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
    fn test_inc_HL() {
        let mut mem = init_mem();
        let mut cpu = CPUState {
            reg: [0, 0, 0, 0, 0, 0x01, FL_Z | FL_N | FL_H | FL_C, 0x01],
            ..INITIAL
        };
        
        let initial:Byte = 0x0E;
        mem[cpu.HL()] = initial;
        cpu = inc_HL(cpu, &mut mem);

        assert_eq!(mem[cpu.HL()], initial+1);
        assert_eq!(cpu.reg[FLAGS], FL_C); // FL_C remains untouched by this operation

        // increment again, this time 0x0F should half-carry into 0x10
        cpu = inc_HL(cpu, &mut mem);
        assert_eq!(mem[cpu.HL()], initial+2);
        assert_eq!(cpu.reg[FLAGS], FL_H | FL_C); // FL_H from half-carry

        // reset value to 0xFF, confirm we get a FL_Z flag on overflow
        mem[cpu.HL()] = 0xFF;
        cpu = inc_HL(cpu, &mut mem);
        assert_eq!(mem[cpu.HL()], 0);
        assert_eq!(cpu.reg[FLAGS], FL_Z | FL_H | FL_C); // todo: should FL_H get set here? it does! but should it?
    }

    #[test]
    fn test_call_d16() {
        let mut mem = init_mem();
        let result = call_d16(0x01, 0x02, INITIAL, &mut mem);
        assert_eq!(
            mem[INITIAL.sp - 0],
            hi(INITIAL.adv_pc(3).pc),
            "failed high check"
        );
        assert_eq!(
            mem[INITIAL.sp - 1],
            lo(INITIAL.adv_pc(3).pc),
            "failed low check"
        );
        assert_eq!(result.pc, 0x0201, "failed sp check")
    }

    #[test]
    fn test_inc_dec() {
        let cpu = CPUState {
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
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0x12, FL_C, 0x11],
            ..INITIAL
        };
        let mut mem = init_mem();
        mem[cpu.HL()] = cpu.reg[REG_L];

        assert_eq!(cp_b(cpu).reg[FLAGS], FL_N);
        assert_eq!(cp_c(cpu).reg[FLAGS], FL_N);
        assert_eq!(cp_d(cpu).reg[FLAGS], FL_N | FL_H);
        assert_eq!(cp_e(cpu).reg[FLAGS], FL_N | FL_H);
        assert_eq!(cp_h(cpu).reg[FLAGS], FL_Z | FL_N);
        assert_eq!(cp_l(cpu).reg[FLAGS], FL_N | FL_H | FL_C);
        assert_eq!(cp_a(cpu).reg[FLAGS], FL_Z | FL_N);
        assert_eq!(cp_d8(cpu, 0x12).reg[FLAGS], FL_N | FL_H | FL_C);
        assert_eq!(cp_HL(cpu, &mem).reg[FLAGS], FL_N | FL_H | FL_C);
    }

    #[test]
    fn test_sub() {
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0x12, FL_C, 0x11],
            ..INITIAL
        };
        assert_eq!(sub_b(cpu).reg[REG_A], 0x11);
        assert_eq!(sub_c(cpu).reg[REG_A], 0x10);
        assert_eq!(sub_d(cpu).reg[REG_A], 0x0F);
        assert_eq!(sub_d(cpu).reg[FLAGS], FL_N | FL_H);
        assert_eq!(sub_e(cpu).reg[REG_A], 0x0E);
        assert_eq!(sub_h(cpu).reg[REG_A], 0x00);
        assert_eq!(sub_h(cpu).reg[FLAGS], FL_Z | FL_N);
        assert_eq!(sub_l(cpu).reg[REG_A], 0xFF);
        assert_eq!(sub_l(cpu).reg[FLAGS], FL_N | FL_H | FL_C);
    }

    #[test]
    fn test_inc16() {
        let cpu = CPUState {
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
        let cpu_c = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0xFF, FL_C, 0x11],
            pc: 0xFF,
            ..INITIAL
        };
        let cpu_z = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0xFF, FL_Z, 0x11],
            ..cpu_c
        };

        assert_eq!(jp_d16(cpu_c, 0x03, 0x02).pc, 0x0203);
        assert_eq!(jp_d16(cpu_c, 0x03, 0x02).tsc, 16);
        assert_eq!(jr_z_r8(cpu_z, 1).pc, cpu_z.adv_pc(2).pc + 1);
        assert_eq!(jr_z_r8(cpu_z, -0xF).pc, cpu_z.adv_pc(2).pc - 0xF);
        assert_eq!(jr_z_r8(cpu_c, 1).pc, cpu_c.adv_pc(2).pc);
        assert_eq!(jr_nz_r8(cpu_c, 1).pc, cpu_c.adv_pc(2).pc + 1);
        assert_eq!(jr_nz_r8(cpu_z, 1).pc, cpu_z.adv_pc(2).pc);
        assert_eq!(jr_nz_r8(cpu_z, 1).tsc, cpu_z.tsc + 8);

        assert_eq!(jr_c_r8(cpu_c, 1).pc, cpu_c.adv_pc(2).pc + 1);
        assert_eq!(jr_c_r8(cpu_z, 1).pc, cpu_z.adv_pc(2).pc);
        assert_eq!(jr_c_r8(cpu_c, 1).tsc, cpu_c.tsc + 12);
        assert_eq!(jr_c_r8(cpu_z, 1).tsc, cpu_z.tsc + 8);

        assert_eq!(jr_nc_r8(cpu_c, 1).pc, cpu_c.adv_pc(2).pc);
        assert_eq!(jr_nc_r8(cpu_z, 1).pc, cpu_z.adv_pc(2).pc + 1);
        assert_eq!(jr_nc_r8(cpu_c, 1).tsc, cpu_c.tsc + 8);
        assert_eq!(jr_nc_r8(cpu_z, 1).tsc, cpu_z.tsc + 12);
    }

    #[test]
    fn test_ld_HL_d8() {
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0xFF, FL_C, 0xAA],
            ..INITIAL
        };
        let mut mem = init_mem();
        impl_ld_HL_d8(cpu, &mut mem, 0x22);
        assert_eq!(mem[cpu.HL()], 0x22);
    }

    #[test]
    fn test_ldi() {
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0x22, FL_C, 0xAA],
            ..INITIAL
        };
        let mut mem = init_mem();
        mem[cpu.HL()] = 0x0F;
        assert_eq!(ldi_HL_a(cpu, &mut mem).reg[REG_H], cpu.reg[REG_H]);
        assert_eq!(ldi_HL_a(cpu, &mut mem).reg[REG_L], cpu.reg[REG_L] + 1);
        assert_eq!(mem[cpu.HL()], cpu.reg[REG_A]);
    }

    #[test]
    fn test_ldd() {
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0x22, FL_C, 0xAA],
            ..INITIAL
        };
        let mut mem = init_mem();
        mem[cpu.HL()] = 0x0F;
        assert_eq!(ldd_HL_a(cpu, &mut mem).reg[REG_H], cpu.reg[REG_H]);
        assert_eq!(ldd_HL_a(cpu, &mut mem).reg[REG_L], cpu.reg[REG_L] - 1);
        assert_eq!(mem[cpu.HL()], cpu.reg[REG_A]);
    }

    #[test]
    fn test_push() {
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0xFF, FL_C, 0xAA],
            ..INITIAL
        };
        let mut mem = init_mem();
        assert_eq!(push_bc(cpu, &mut mem).sp, cpu.sp - 2);
        assert_eq!(mem[cpu.sp - 1], cpu.reg[REG_B]);
        assert_eq!(mem[cpu.sp - 2], cpu.reg[REG_C]);
    }

    #[test]
    fn test_pop() {
        let cpu = CPUState {
            sp: 0xDEAD,
            ..INITIAL
        };
        let mut mem = init_mem();
        mem[0xDEAD] = 0xAD;
        mem[0xDEAD + 1] = 0xDE;

        assert_eq!(pop_bc(cpu, &mem).sp, cpu.sp + 2);
        assert_eq!(pop_bc(cpu, &mem).reg[REG_B], 0xDE);
        assert_eq!(pop_bc(cpu, &mem).reg[REG_C], 0xAD);
    }

    #[test]
    fn test_ret() {
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0xCC, 0x02, 0x03, 0x11, 0xFF, FL_C, 0xAA],
            sp: 0xFFFC,
            ..INITIAL
        };
        let mut mem = init_mem();
        mem[0xFFFE] = 0xBE;
        mem[0xFFFD] = 0xEF;
        assert_eq!(ret(cpu, &mem).pc, 0xBEEF);
        assert_eq!(ret(cpu, &mem).sp, 0xFFFE);
    }

    #[test]
    fn test_16b_loads() {
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0xBB, 0xCC, 0xDD, 0xEE, 0x11, 0x22, FL_C, 0xAA],
            ..INITIAL
        };
        let mut mem = init_mem();
        mem[0xBBCC] = 0xAB;
        mem[0xDDEE] = 0xAD;
        assert_eq!(ld_a_BC(cpu, &mem).reg[REG_A], mem[0xBBCC]);
        assert_eq!(ld_a_DE(cpu, &mem).reg[REG_A], mem[0xDDEE]);

        ld_BC_a(cpu, &mut mem);
        assert_eq!(mem[0xBBCC], 0xAA);

        ld_DE_a(cpu, &mut mem);
        assert_eq!(mem[0xDDEE], 0xAA);

        ld_A16_a(0xCE, 0xFA, cpu, &mut mem);
        assert_eq!(mem[0xFACE], 0xAA);
    }

    #[test]
    fn test_FF00_offsets() {
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0xCC, 0x02, 0x03, 0x11, 0xFF, FL_C, 0xAA],
            ..INITIAL
        };
        let mut mem = init_mem();
        mem[0xFF00] = 0;
        mem[0xFF01] = 1;
        mem[0xFF02] = 2;
        mem[0xFF03] = 3;
        mem[0xFFCC] = 0xCC;
        assert_eq!(ld_a_FF00_A8(cpu, &mem, 0x02).reg[REG_A], 0x02);
        assert_eq!(ld_a_FF00_C(cpu, &mem).reg[REG_A], 0xCC);
        ld_FF00_A8_a(0x01, cpu, &mut mem);
        assert_eq!(mem[0xFF01], cpu.reg[REG_A]);

        ld_FF00_C_a(cpu, &mut mem);
        assert_eq!(mem[0xFFCC], cpu.reg[REG_A]);
    }

    #[test]
    fn test_rotations() {
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0, 0x80],
            ..INITIAL
        };
        // single rotation, store in carry if MSB is set
        assert_eq!(rlca(cpu).reg[REG_A], 0x01);
        assert_eq!(rlca(cpu).reg[FLAGS], FL_C);

        // single rotation through carry
        assert_eq!(rla(cpu).reg[REG_A], 0x00);
        assert_eq!(rla(cpu).reg[FLAGS], FL_C);

        // double rotation through carry, carry should shift back down
        assert_eq!(rla(rla(cpu)).reg[REG_A], 0x01);
        assert_eq!(rla(rla(cpu)).reg[FLAGS], 0x00);

        assert_eq!(impl_rl_r(cpu, REG_B).reg[REG_B], 0x80);
        assert_eq!(impl_rl_r(cpu, REG_C).reg[REG_C], 0x80);
        assert_eq!(impl_rl_r(cpu, REG_D).reg[REG_D], 0x80);
        assert_eq!(impl_rl_r(cpu, REG_E).reg[REG_E], 0x80);
        assert_eq!(impl_rl_r(cpu, REG_H).reg[REG_H], 0x80);
        assert_eq!(impl_rl_r(cpu, REG_L).reg[REG_L], 0x80);
        assert_eq!(impl_rl_r(cpu, REG_A).reg[REG_A], 0x00);
        assert_eq!(impl_rl_r(cpu, REG_A).reg[FLAGS], FL_Z | FL_C);
        assert_eq!(impl_rl_r(impl_rl_r(cpu, REG_A), REG_A).reg[REG_A], 0x01);
    }

    #[test]
    fn test_bit() {
        let cpu = CPUState {
            //    B          C       D       E       H       L      fl     A
            reg: [1 << 0, 1 << 1, 1 << 2, 1 << 3, 1 << 4, 1 << 5, FL_C, 1 << 7],
            ..INITIAL
        };
        assert_eq!(impl_bit(cpu, 7, REG_H).reg[FLAGS], FL_H | cpu.reg[FLAGS]);
        assert_eq!(impl_set(cpu, 7, REG_H).reg[REG_H], cpu.reg[REG_H] | 0x80);
    }

    #[test]
    fn test_timers() {
        let mut mem = init_mem();
        mem[TIMA] = 0;
        mem[TMA] = 0;
        mem[TAC] = 0;
        assert_eq!(tac_enabled(&mem), false);
        mem[TAC] = 0b100; // (enabled, 1024 cycles per tick)
        assert_eq!(tac_enabled(&mem), true);

        let new_timers = update_clocks(HardwareTimers::new(), &mut mem, 1024);
        assert_eq!(new_timers.timer, 0);
        assert_eq!(mem[TIMA], 1);

        tima_reset(&mut mem);
        assert_eq!(mem[TIMA], 0);

        mem[TAC] = 0b111; // (enabled, 256 cycles per tick)
        let new_timers = update_clocks(HardwareTimers::new(), &mut mem, 1024);
        assert_eq!(new_timers.timer, 0);
        assert_eq!(mem[TIMA], 4);

        mem[TMA] = 0xFF;
        tima_reset(&mut mem);
        assert_eq!(mem[TIMA], mem[TMA]);

        mem[TMA] = 0xAA;
        assert_ne!(mem[IF], FL_INT_TIMER);
        let _even_newer_timers = update_clocks(new_timers, &mut mem, 256);
        // should have overflowed as we just set it to 0xFF moments ago
        assert_eq!(mem[TIMA], 0xAA);
        assert_eq!(mem[IF], FL_INT_TIMER);

        // TODO test DIV
        // TODO can we test frame timer? it's set up differently...
    }

    #[test]
    fn test_lcd() {
        let mut mem = init_mem();
        set_lcd_mode(3, &mut mem);
        assert_eq!(lcd_mode(&mem), 3);
    }

    #[test]
    fn test_impl_rlc_r() {
        let cpu = CPUState { 
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x80, 0x03, 0x11, 0xFF, FL_C, 0xAA],
            ..INITIAL 
        };

        let rot_b = impl_rlc_r(cpu, REG_B);
        assert_eq!(rot_b.reg[REG_B], 0x00);
        assert_eq!(rot_b.reg[FLAGS], FL_Z);

        let rot_c = impl_rlc_r(cpu, REG_C);
        assert_eq!(rot_c.reg[REG_C], 0x02);
        assert_eq!(rot_c.reg[FLAGS], 0x00);

        let rot_d = impl_rlc_r(cpu, REG_D);
        assert_eq!(rot_d.reg[REG_D], 0x01);
        assert_eq!(rot_d.reg[FLAGS], FL_C);

        let rot_l = impl_rlc_r(cpu, REG_L);
        assert_eq!(rot_l.reg[REG_L], 0xFF);
        assert_eq!(rot_l.reg[FLAGS], FL_C);
    }
}
