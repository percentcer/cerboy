#![allow(non_snake_case)]
#![allow(dead_code)]

pub mod cpu {
    use crate::bits::*;
    use crate::decode::*;
    use crate::memory::*;
    use crate::types::*;

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

    pub const GB_SCREEN_WIDTH: usize = 160;
    pub const GB_SCREEN_HEIGHT: usize = 144;

    // classic gameboy only has four shades, white (00), light (01), dark (10), black (11)
    pub const PAL_CLASSIC: [u32; 4] = [0xE0F8D0, 0x88C070, 0x346856, 0x081820];
    pub const PAL_ICE_CREAM: [u32; 4] = [0xFFF6D3, 0xF9A875, 0xEB6B6F, 0x7C3F58];
    pub const PAL_VBOY: [u32; 4] = [0xEF0000, 0xA40000, 0x550000, 0x000000];

    pub fn palette_lookup(color: Byte, plt: Byte, lut: &[u32; 4]) -> u32 {
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
    pub const TICKS_PER_OAM_SEARCH: u64 = 80;
    pub const TICKS_PER_VRAM_IO: u64 = 168; // roughly
    pub const TICKS_PER_HBLANK: u64 = 208; // roughly
    pub const TICKS_PER_SCANLINE: u64 = TICKS_PER_OAM_SEARCH + TICKS_PER_VRAM_IO + TICKS_PER_HBLANK;
    pub const TICKS_PER_VBLANK: u64 = TICKS_PER_SCANLINE * 10; // 144 on screen + 10 additional lines
    pub const TICKS_PER_FRAME: u64 = (TICKS_PER_SCANLINE * GB_SCREEN_HEIGHT as u64) + TICKS_PER_VBLANK; // 70224 cycles

    pub const TICKS_PER_DIV_INC: u64 = 256;

    // tile constants
    pub const BYTES_PER_TILE: u16 = 16;

    // interrupt flags
    pub const FL_INT_VBLANK: Byte = 1 << 0;
    pub const FL_INT_STAT: Byte = 1 << 1;
    pub const FL_INT_TIMER: Byte = 1 << 2;
    pub const FL_INT_SERIAL: Byte = 1 << 3;
    pub const FL_INT_JOYPAD: Byte = 1 << 4;

    #[derive(Copy, Clone, Debug)]
    pub struct CPUState {
        // ------------ meta, not part of actual gb hardware but useful
        pub tsc: u64, // counting cycles since reset
        inst_count: u64, // counting instructions since reset
        inst_ei: u64, // timestamp when ei was set, used to keep track of the two-instruction-delay
        // ------------ hardware
        pub(crate) reg: [Byte; 8],
        pub(crate) sp: Word,
        pub(crate) pc: Word,
        ime: bool, // true == interrupts enabled
    }

    impl CPUState {
        /// Initializes a new CPUState struct
        ///
        /// Starting values should match original gb hardware, more here:
        /// https://gbdev.gg8.se/files/docs/mirrors/pandocs.html#powerupsequence
        pub const fn new() -> CPUState {
            CPUState {
                tsc: 0,
                inst_count: 0,
                inst_ei: 0,
                //    B     C     D     E     H     L     fl    A
                reg: [0x00, 0x13, 0x00, 0xD8, 0x01, 0x4D, 0xB0, 0x01],
                sp: 0xFFFE,
                pc: ROM_ENTRY,
                ime: false,
            }
        }

        /// Commonly used for addresses
        ///
        /// Combines the H and L registers into a usize for mem indexing
        const fn HL(&self) -> Word {
            combine(self.reg[REG_H], self.reg[REG_L])
        }
        /// Combines the A and FLAG registers for 16b operations
        const fn AF(&self) -> Word {
            combine(self.reg[REG_A], self.reg[FLAGS])
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

    pub struct HardwareTimers {
        timer: u64,
        divider: u64,
    }

    impl HardwareTimers {
        pub const fn new() -> HardwareTimers {
            HardwareTimers {
                timer: 0,
                divider: 0,
            }
        }
    }

    pub fn update_clocks(state: HardwareTimers, mem: &mut Memory, cycles: u64) -> HardwareTimers {
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

    pub fn next(cpu: CPUState, mem: &mut Memory) -> CPUState {
        // fetch and execute
        // -----------------
        let pc = cpu.pc;
        let cpu = CPUState{ inst_count: cpu.inst_count + 1, ..cpu }; // referenced by interrupt enabling instructions
        // cerboy::decode::print_op(mem[pc]);
        
        // check interrupts
        // -----------------
        // https://gbdev.io/pandocs/single.html#ime-interrupt-master-enable-flag-write-only
        // The effect of EI is delayed by one instruction.
        // This means that EI followed immediately by DI does not
        // allow interrupts between the EI and the DI.
        let ei_valid_delay = (cpu.inst_count - cpu.inst_ei) > 1;
        let enabled_flags = mem[IE] & mem[IF];
        if cpu.ime && enabled_flags != 0 && ei_valid_delay {
            if (enabled_flags & FL_INT_VBLANK) > 0 {
                jump_to_int_vec(cpu, mem, FL_INT_VBLANK, VEC_INT_VBLANK)
            } else if (enabled_flags & FL_INT_STAT) > 0 {
                jump_to_int_vec(cpu, mem, FL_INT_STAT, VEC_INT_STAT)
            } else if (enabled_flags & FL_INT_TIMER) > 0 {
                jump_to_int_vec(cpu, mem, FL_INT_TIMER, VEC_INT_TIMER)
            } else if (enabled_flags & FL_INT_SERIAL) > 0 {
                jump_to_int_vec(cpu, mem, FL_INT_SERIAL, VEC_INT_SERIAL)
            } else if (enabled_flags & FL_INT_JOYPAD) > 0 {
                jump_to_int_vec(cpu, mem, FL_INT_JOYPAD, VEC_INT_JOYPAD)
            } else {
                panic!("interrupt enabled but unknown flag?")
            }
        } else {
            // todo: is this correct? I'm assuming it can't handle an interrupt
            // and then go right into the next instruction, it's one or the other
            let inst = crate::decode::decode(mem[pc]);
        match mem[pc] {
            0x00 => nop(cpu),
            0x01 => ld_bc_d16(cpu, mem[pc + 1], mem[pc + 2]),
            0x02 => ld_BC_a(cpu, mem),
            0x03 => inc_bc(cpu),
            0x04 => inc_b(cpu),
            0x05 => dec_b(cpu),
            0x06 => ld_b_d8(cpu, mem[pc + 1]),
            0x07 => rlca(cpu),
            0x08 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x09 => add_hl_bc(cpu),
            0x0A => ld_a_BC(cpu, &mem),
            0x0B => dec_bc(cpu),
            0x0C => inc_c(cpu),
            0x0D => dec_c(cpu),
            0x0E => ld_c_d8(cpu, mem[pc + 1]),
            0x0F => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x10 => stop(cpu),
            0x11 => ld_de_d16(cpu, mem[pc + 1], mem[pc + 2]),
            0x12 => ld_DE_a(cpu, mem),
            0x13 => inc_de(cpu),
            0x14 => inc_d(cpu),
            0x15 => dec_d(cpu),
            0x16 => ld_d_d8(cpu, mem[pc + 1]),
            0x17 => rla(cpu),
            0x18 => jr_r8(cpu, signed(mem[pc + 1])),
            0x19 => add_hl_de(cpu),
            0x1A => ld_a_DE(cpu, &mem),
            0x1B => dec_de(cpu),
            0x1C => inc_e(cpu),
            0x1D => dec_e(cpu),
            0x1E => ld_e_d8(cpu, mem[pc + 1]),
            0x1F => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x20 => jr_nz_r8(cpu, signed(mem[pc + 1])),
            0x21 => ld_hl_d16(cpu, mem[pc + 1], mem[pc + 2]),
            0x22 => ldi_HL_a(cpu, mem),
            0x23 => inc_hl(cpu),
            0x24 => inc_h(cpu),
            0x25 => dec_h(cpu),
            0x26 => ld_h_d8(cpu, mem[pc + 1]),
            0x27 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x28 => jr_z_r8(cpu, signed(mem[pc + 1])),
            0x29 => add_hl_hl(cpu),
            0x2A => ldi_a_HL(cpu, mem),
            0x2B => dec_hl(cpu),
            0x2C => inc_l(cpu),
            0x2D => dec_l(cpu),
            0x2E => ld_l_d8(cpu, mem[pc + 1]),
            0x2F => cpl(cpu),
            0x30 => jr_nc_r8(cpu, signed(mem[pc + 1])),
            0x31 => ld_sp_d16(cpu, mem[pc + 1], mem[pc + 2]),
            0x32 => ldd_HL_a(cpu, mem),
            0x33 => inc_sp(cpu),
            0x34 => inc_HL(cpu, mem),
            0x35 => dec_HL(cpu, mem),
            0x36 => ld_HL_d8(cpu, mem[pc + 1], mem),
            0x37 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x38 => jr_c_r8(cpu, signed(mem[pc + 1])),
            0x39 => add_hl_sp(cpu),
            0x3A => ldd_a_HL(cpu, mem),
            0x3B => dec_sp(cpu),
            0x3C => inc_a(cpu),
            0x3D => dec_a(cpu),
            0x3E => ld_a_d8(cpu, mem[pc + 1]),
            0x3F => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
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
            0x70 => ld_HL_b(cpu, mem),
            0x71 => ld_HL_c(cpu, mem),
            0x72 => ld_HL_d(cpu, mem),
            0x73 => ld_HL_e(cpu, mem),
            0x74 => ld_HL_h(cpu, mem),
            0x75 => ld_HL_l(cpu, mem),
            0x76 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x77 => ld_HL_a(cpu, mem),
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
            0x98 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x99 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x9A => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x9B => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x9C => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x9D => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x9E => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0x9F => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
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
            0xC2 => jp_f_d16(cpu, mem[pc + 1], mem[pc + 2], 0xC2),
            0xC3 => jp_d16(cpu, mem[pc + 1], mem[pc + 2]),
            0xC4 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xC5 => push_bc(cpu, mem),
            0xC6 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xC7 => rst_n(cpu, mem, 0xC7),
            0xC8 => ret_z(cpu, &mem),
            0xC9 => ret(cpu, &mem),
            0xCA => jp_f_d16(cpu, mem[pc + 1], mem[pc + 2], 0xCA),
            0xCB => {
                let op_cb = mem[pc + 1];
                if ((op_cb & 0xF) == 0x6) || ((op_cb & 0xF) == 0xE) {
                    panic!("[HL] instructions not yet implemented");
                }
                let icb = decodeCB(op_cb);
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
                    "RES" => impl_res_n_r(cpu, icb.bit, icb.reg),
                    "SET" => impl_set(cpu, icb.bit, icb.reg),
                    _ => panic!("unknown instruction (0xCB) 0x{:X} ({})", op_cb, icb.opcode)
                }
            },
            0xCC => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xCD => call_d16(mem[pc + 1], mem[pc + 2], cpu, mem),
            0xCE => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xCF => rst_n(cpu, mem, 0xCF),
            0xD0 => ret_nc(cpu, &mem),
            0xD1 => pop_de(cpu, &mem),
            0xD2 => jp_f_d16(cpu, mem[pc + 1], mem[pc + 2], 0xD2),
            0xD3 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xD4 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xD5 => push_de(cpu, mem),
            0xD6 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xD7 => rst_n(cpu, mem, 0xD7),
            0xD8 => ret_c(cpu, &mem),
            0xD9 => reti(cpu, &mem),
            0xDA => jp_f_d16(cpu, mem[pc + 1], mem[pc + 2], 0xDA),
            0xDB => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xDC => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xDD => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xDE => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xDF => rst_n(cpu, mem, 0xDF),
            0xE0 => ld_FF00_A8_a(mem[pc + 1], cpu, mem),
            0xE1 => pop_hl(cpu, &mem),
            0xE2 => ld_FF00_C_a(cpu, mem),
            0xE3 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xE4 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xE5 => push_hl(cpu, mem),
            0xE6 => and_d8(cpu, mem[pc + 1]),
            0xE7 => rst_n(cpu, mem, 0xE7),
            0xE8 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xE9 => jp_hl(cpu),
            0xEA => ld_A16_a(mem[pc + 1], mem[pc + 2], cpu, mem),
            0xEB => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xEC => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xED => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xEE => xor_d8(cpu, mem[pc + 1]),
            0xEF => rst_n(cpu, mem, 0xEF),
            0xF0 => ld_a_FF00_A8(cpu, &mem, mem[pc + 1]),
            0xF1 => pop_af(cpu, &mem),
            0xF2 => ld_a_FF00_C(cpu, &mem),
            0xF3 => di(cpu),
            0xF4 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xF5 => push_af(cpu, mem),
            0xF6 => or_d8(cpu, mem[pc + 1]),
            0xF7 => rst_n(cpu, mem, 0xF7),
            0xF8 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xF9 => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xFA => ld_a_A16(mem[pc + 1], mem[pc + 2], cpu, &mem),
            0xFB => ei(cpu),
            0xFC => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xFD => panic!("unknown instruction 0x{:X} ({})", mem[pc], inst.mnm),
            0xFE => cp_d8(cpu, mem[pc + 1]),
            0xFF => rst_n(cpu, mem, 0xFF),
        }
        }
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
            reg,
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

    fn impl_push_rr(cpu: CPUState, mem: &mut Memory, reg_high: usize, reg_low: usize) -> CPUState {
        mem[cpu.sp - 0] = cpu.reg[reg_high];
        mem[cpu.sp - 1] = cpu.reg[reg_low];
        CPUState {
            pc: cpu.pc + 1,
            tsc: cpu.tsc + 16,
            sp: cpu.sp - 2,
            ..cpu
        }
    }

    fn impl_pop_rr(cpu: CPUState, mem: &Memory, reg_high: usize, reg_low: usize) -> CPUState {
        let mut reg = cpu.reg;
        reg[reg_high] = mem[cpu.sp + 2];
        reg[reg_low] = mem[cpu.sp + 1];
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
    fn push_bc(cpu: CPUState, mem: &mut Memory) -> CPUState {
        impl_push_rr(cpu, mem, REG_B, REG_C)
    }
    fn push_de(cpu: CPUState, mem: &mut Memory) -> CPUState {
        impl_push_rr(cpu, mem, REG_D, REG_E)
    }
    fn push_hl(cpu: CPUState, mem: &mut Memory) -> CPUState {
        impl_push_rr(cpu, mem, REG_H, REG_L)
    }
    fn push_af(cpu: CPUState, mem: &mut Memory) -> CPUState {
        impl_push_rr(cpu, mem, REG_A, FLAGS)
    }

    //   pop  rr          x1        12 (AF) rr=(SP)  SP=SP+2   (rr may be BC,DE,HL,AF)
    // ----------------------------------------------------------------------------
    fn pop_bc(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_pop_rr(cpu, mem, REG_B, REG_C)
    }
    fn pop_de(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_pop_rr(cpu, mem, REG_D, REG_E)
    }
    fn pop_hl(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_pop_rr(cpu, mem, REG_H, REG_L)
    }
    fn pop_af(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_pop_rr(cpu, mem, REG_A, FLAGS)
    } // note that this one writes to flags

    // GMB 8bit-Arithmetic/logical Commands
    // ============================================================================
    const fn impl_add(cpu: CPUState, arg: Byte) -> CPUState {
        // z0hc
        let mut reg = cpu.reg;
        let reg_a: Byte = cpu.reg[REG_A];

        let h: bool = ((reg_a & 0x0f) + (arg & 0x0f)) & 0x10 > 0;
        let (result, c) = reg_a.overflowing_add(arg);
        let flags: Byte = if result == 0 { FL_Z } else { 0 }
            | if h { FL_H } else { 0 }
            | if c { FL_C } else { 0 };
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
        let (h, (res, _c)) = (
            mem[cpu.HL()] & 0x0F == 0x0F,
            mem[cpu.HL()].overflowing_add(1),
        );

        let flags = reg[FLAGS] & FL_C // maintain the carry, we'll set the rest
    | if res == 0x00 {FL_Z} else {0}
    | if h {FL_H} else {0};
        reg[FLAGS] = flags;

        mem[cpu.HL()] = res;

        CPUState { reg, ..cpu }.adv_pc(1).tick(12)
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
    // ----------------------------------------------------------------------------
    fn dec_HL(cpu: CPUState, mem: &mut Memory) -> CPUState {
        let mut reg = cpu.reg;
        let (h, (res, _c)) = (
            mem[cpu.HL()] & 0x0F == 0x00,
            mem[cpu.HL()].overflowing_sub(1),
        );

        let flags = reg[FLAGS] & FL_C // maintain the carry, we'll set the rest
    | if res == 0x00 {FL_Z} else {0}
    | if h {FL_H} else {0};
        reg[FLAGS] = flags;

        mem[cpu.HL()] = res;

        CPUState { reg, ..cpu }.adv_pc(1).tick(12)
    }

    //   daa              27         4 z-0x decimal adjust akku

    //   cpl              2F         4 -11- A = A xor FF
    // ----------------------------------------------------------------------------
    const fn cpl(cpu: CPUState) -> CPUState {
        let mut reg = cpu.reg;
        reg[REG_A] = reg[REG_A] ^ 0xFF;
        reg[FLAGS] = (reg[FLAGS] & FL_Z) | FL_N | FL_H | (reg[FLAGS] & FL_C);
        CPUState { reg, ..cpu }.adv_pc(1).tick(4)
    }

    // GMB 16bit-Arithmetic/logical Commands
    // ============================================================================

    //   add  HL,rr     x9           8 -0hc HL = HL+rr     ;rr may be BC,DE,HL,SP
    // ----------------------------------------------------------------------------
    const fn impl_add_hl_rr(cpu: CPUState, rr: Word) -> CPUState {
        let mut reg = cpu.reg;

        let h: bool = ((cpu.reg[REG_L] & 0x0f) + (lo(rr) & 0x0f)) & 0x10 > 0;
        let (result, c) = cpu.HL().overflowing_add(rr);

        reg[FLAGS] = (reg[FLAGS] & FL_Z) | if h { FL_H } else { 0 } | if c { FL_C } else { 0 };
        reg[REG_H] = hi(result);
        reg[REG_L] = lo(result);

        CPUState { reg, ..cpu }.adv_pc(1).tick(8)
    }

    const fn add_hl_bc(cpu: CPUState) -> CPUState {
        impl_add_hl_rr(cpu, cpu.BC())
    }
    const fn add_hl_de(cpu: CPUState) -> CPUState {
        impl_add_hl_rr(cpu, cpu.DE())
    }
    const fn add_hl_hl(cpu: CPUState) -> CPUState {
        impl_add_hl_rr(cpu, cpu.HL())
    }
    const fn add_hl_sp(cpu: CPUState) -> CPUState {
        impl_add_hl_rr(cpu, cpu.sp)
    }

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
        let fl_c = if (result & 1) > 0 { FL_C } else { 0 };

        reg[dst] = result;
        reg[FLAGS] = fl_z(result) | fl_c;

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
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

        reg[FLAGS] =
            if (cpu.reg[dst] & mask) > 0 { FL_Z } else { 0 } | FL_H | (cpu.reg[FLAGS] & FL_C);
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
    // ----------------------------------------------------------------------------
    const fn impl_res_n_r(cpu: CPUState, n: Byte, r: usize) -> CPUState {
        let mut reg = cpu.reg;
        let mask = 1 << n;

        reg[r] &= !mask;

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    #[test]
    fn test_impl_res_n_r() {
        let cpu = CPUState {
            reg: [0xFE, 0, 0, 0, 0, 0, 0, 0],
            ..CPUState::new()
        };
        assert_eq!(impl_res_n_r(cpu, 0, REG_B).reg[REG_B], 0b11111110);
        assert_eq!(impl_res_n_r(cpu, 1, REG_B).reg[REG_B], 0b11111100);
        assert_eq!(impl_res_n_r(cpu, 2, REG_B).reg[REG_B], 0b11111010);
        assert_eq!(impl_res_n_r(cpu, 3, REG_B).reg[REG_B], 0b11110110);
        assert_eq!(impl_res_n_r(cpu, 4, REG_B).reg[REG_B], 0b11101110);
        assert_eq!(impl_res_n_r(cpu, 5, REG_B).reg[REG_B], 0b11011110);
        assert_eq!(impl_res_n_r(cpu, 6, REG_B).reg[REG_B], 0b10111110);
        assert_eq!(impl_res_n_r(cpu, 7, REG_B).reg[REG_B], 0b01111110);
    }

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
    // ----------------------------------------------------------------------------
    const fn stop(cpu: CPUState) -> CPUState {
        // todo: not sure what to do here
        cpu.adv_pc(2).tick(0)
    }

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
            inst_ei: cpu.inst_count,
            ..cpu.adv_pc(1).tick(4)
        }
    }

    // GMB Jumpcommands
    // ============================================================================
    const fn impl_jp(cpu: CPUState, addr: Word) -> CPUState {
        CPUState { pc: addr, ..cpu }
    }

    const fn impl_jr(cpu: CPUState, arg: SByte) -> CPUState {
        CPUState {
            pc: cpu.pc.wrapping_add(arg as Word),
            ..cpu
        }
    }

    //   jp   nn        C3 nn nn    16 ---- jump to nn, PC=nn
    // ----------------------------------------------------------------------------
    const fn jp_d16(cpu: CPUState, low: Byte, high: Byte) -> CPUState {
        impl_jp(cpu, combine(high, low)).tick(16)
    }

    //   jp   HL        E9           4 ---- jump to HL, PC=HL
    // ----------------------------------------------------------------------------
    const fn jp_hl(cpu: CPUState) -> CPUState {
        impl_jp(cpu, cpu.HL()).tick(4)
    }

    #[test]
    fn test_jp_hl() {
        let cpu = CPUState::new();
        assert_eq!(jp_hl(cpu).pc, cpu.HL())
    }

    //   jp   f,nn      xx nn nn 16;12 ---- conditional jump if nz,z,nc,c
    // ----------------------------------------------------------------------------
    const fn jp_f_d16(cpu: CPUState, low: Byte, high: Byte, op: Byte) -> CPUState {
        // 0xC2: NZ | 0xD2: NC | 0xCA: Z | 0xDA: C
        let do_jump = match op {
            0xC2 => (cpu.reg[FLAGS] & FL_Z) == 0,
            0xD2 => (cpu.reg[FLAGS] & FL_C) == 0,
            0xCA => (cpu.reg[FLAGS] & FL_Z) != 0,
            0xDA => (cpu.reg[FLAGS] & FL_C) != 0,
            _ => panic!("jp_f_d16 unreachable"),
        };
        if do_jump {
            impl_jp(cpu, combine(high, low)).tick(16)
        } else {
            cpu.adv_pc(3).tick(12)
        }
    }

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
            inst_ei: cpu.inst_count,
            // except for the ime change, reti is identical to ret
            ..ret(cpu, mem)
        }
    }

    //   rst  n         xx          16 ---- call to 00,08,10,18,20,28,30,38
    // ----------------------------------------------------------------------------
    fn rst_n(cpu: CPUState, mem: &mut Memory, opcode: Byte) -> CPUState {
        let cpu = cpu.adv_pc(1).tick(16);
        let rst_hi = (opcode & HIGH_MASK_NIB) - 0xC0;
        let rst_lo = opcode & 0x08;
        let rst_addr = rst_hi | rst_lo;

        mem[cpu.sp - 0] = hi(cpu.pc);
        mem[cpu.sp - 1] = lo(cpu.pc);
        CPUState {
            sp: cpu.sp - 2,
            pc: rst_addr as Word,
            ..cpu
        }
    }

    #[test]
    fn test_rst_n() {
        let cpu = CPUState::new();
        let mut mem = Memory::new();

        assert_eq!(rst_n(cpu, &mut mem, 0xC7).pc, VEC_RST_00);
        assert_eq!(rst_n(cpu, &mut mem, 0xD7).pc, VEC_RST_10);
        assert_eq!(rst_n(cpu, &mut mem, 0xE7).pc, VEC_RST_20);
        assert_eq!(rst_n(cpu, &mut mem, 0xF7).pc, VEC_RST_30);

        assert_eq!(rst_n(cpu, &mut mem, 0xCF).pc, VEC_RST_08);
        assert_eq!(rst_n(cpu, &mut mem, 0xDF).pc, VEC_RST_18);
        assert_eq!(rst_n(cpu, &mut mem, 0xEF).pc, VEC_RST_28);
        assert_eq!(rst_n(cpu, &mut mem, 0xFF).pc, VEC_RST_38);
    }

    // ============================================================================
    // interrupts
    // ============================================================================
    fn jump_to_int_vec(cpu: CPUState, mem: &mut Memory, fl_int: Byte, vec_int: Word) -> CPUState {
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
    pub fn request_interrupt(mem: &mut Memory, int_flag: Byte) {
        mem[IF] |= int_flag;
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

    pub fn lcd_mode(mem: &Memory) -> Byte {
        mem[STAT] & 0b11
    }

    pub fn set_lcd_mode(mode: Byte, mem: &mut Memory) {
        mem[STAT] = ((mem[STAT] >> 2) << 2) | (mode & 0b11);
    }
}

pub mod memory {
    use crate::types::*;
    use std::{
        ops::{Index, IndexMut},
        str::from_utf8,
    };

    // 0000-3FFF   16KB ROM Bank 00     (in cartridge, fixed at bank 00)
    pub const MEM_BANK_00: Word = 0x0000;
    // 4000-7FFF   16KB ROM Bank 01..NN (in cartridge, switchable bank number)
    pub const MEM_BANK_NN: Word = 0x4000;
    // 8000-9FFF   8KB Video RAM (VRAM) (switchable bank 0-1 in CGB Mode)
    pub const MEM_VRAM: Word = 0x8000;
    // A000-BFFF   8KB External RAM     (in cartridge, switchable bank, if any)
    pub const MEM_EXT: Word = 0xA000;
    // C000-CFFF   4KB Work RAM Bank 0 (WRAM)
    pub const MEM_WRAM_0: Word = 0xC000;
    // D000-DFFF   4KB Work RAM Bank 1 (WRAM)  (switchable bank 1-7 in CGB Mode)
    pub const MEM_WRAM_1: Word = 0xD000;
    // E000-FDFF   Same as C000-DDFF (ECHO)    (typically not used)
    pub const MEM_ECHO: Word = 0xE000;
    // FE00-FE9F   Sprite Attribute Table (OAM)
    pub const MEM_OAM: Word = 0xFE00;
    // FEA0-FEFF   Not Usable
    pub const MEM_NOT_USABLE: Word = 0xFEA0;
    // FF00-FF7F   I/O Ports
    pub const MEM_IO_PORTS: Word = 0xFF00;
    // FF80-FFFE   High RAM (HRAM)
    pub const MEM_HRAM: Word = 0xFF80;
    // FFFF        Interrupt Enable Register

    // RST locations (vectors)
    pub const VEC_RST_00: Word = 0x0000;
    pub const VEC_RST_08: Word = 0x0008;
    pub const VEC_RST_10: Word = 0x0010;
    pub const VEC_RST_18: Word = 0x0018;
    pub const VEC_RST_20: Word = 0x0020;
    pub const VEC_RST_28: Word = 0x0028;
    pub const VEC_RST_30: Word = 0x0030;
    pub const VEC_RST_38: Word = 0x0038;

    // Interrupt locations (vectors)
    pub const VEC_INT_VBLANK: Word = 0x0040;
    pub const VEC_INT_STAT: Word = 0x0048;
    pub const VEC_INT_TIMER: Word = 0x0050;
    pub const VEC_INT_SERIAL: Word = 0x0058;
    pub const VEC_INT_JOYPAD: Word = 0x0060;
    // named I/O memory locations [FF00..FF7F]
    pub const JOYP: Word = 0xFF00;
    // timers
    pub const DIV: Word = 0xFF04;
    pub const TIMA: Word = 0xFF05;
    pub const TMA: Word = 0xFF06;
    pub const TAC: Word = 0xFF07;
    // audio
    pub const NR10: Word = 0xFF10;
    pub const NR11: Word = 0xFF11;
    pub const NR12: Word = 0xFF12;
    pub const NR14: Word = 0xFF14;
    pub const NR21: Word = 0xFF16;
    pub const NR22: Word = 0xFF17;
    pub const NR24: Word = 0xFF19;
    pub const NR30: Word = 0xFF1A;
    pub const NR31: Word = 0xFF1B;
    pub const NR32: Word = 0xFF1C;
    pub const NR33: Word = 0xFF1E;
    pub const NR41: Word = 0xFF20;
    pub const NR42: Word = 0xFF21;
    pub const NR43: Word = 0xFF22;
    pub const NR44: Word = 0xFF23;
    pub const NR50: Word = 0xFF24;
    pub const NR51: Word = 0xFF25;
    pub const NR52: Word = 0xFF26;
    // rendering
    pub const LCDC: Word = 0xFF40;
    pub const STAT: Word = 0xFF41;
    pub const SCY: Word = 0xFF42;
    pub const SCX: Word = 0xFF43;
    pub const LY: Word = 0xFF44;
    pub const LYC: Word = 0xFF45;
    pub const DMA: Word = 0xFF46; // <-- OAM memory transfer
    pub const BGP: Word = 0xFF47;
    pub const OBP0: Word = 0xFF48;
    pub const OBP1: Word = 0xFF49;
    pub const WY: Word = 0xFF4A;
    pub const WX: Word = 0xFF4B;
    // interrupt registers
    pub const IF: Word = 0xFF0F;
    pub const IE: Word = 0xFFFF;

    // sizes
    pub const KB: usize = 0x0400; // one kilobyte
    pub const CART_SIZE_MAX: usize = 0x200000;
    pub const MEM_SIZE: usize = 0xFFFF + 1;
    pub const BANK_SIZE: usize = 0x4000;

    // ROM Header
    pub const ROM_ENTRY: Word = 0x0100;
    pub const ROM_LOGO: Word = 0x0104;
    pub const ROM_TITLE: Word = 0x0134;
    pub const ROM_TITLE_END: Word = 0x0143 + 1;
    pub const ROM_MFR_CODE: Word = 0x013F;
    pub const ROM_SGB: Word = 0x0146;
    pub const ROM_TYPE: Word = 0x0147;
    pub const ROM_SIZE: Word = 0x0148;
    pub const ROM_RAM_SIZE: Word = 0x0149;
    pub const ROM_DESTINATION: Word = 0x014A;

    pub struct Cartridge(Box<[Byte]>);
    impl Cartridge {
        // todo: CGB flag
        // todo: MFR codes
        // todo: Licensee codes
        // todo: SGB flag
        // todo: Old Licensee code
        // todo: Mask rom version number
        // todo: Checksum
        // todo: Checksum (Global)
        pub fn new(rom_path: &str) -> Cartridge {
            let rom: Vec<Byte> = crate::io::read_bytes(rom_path);
            Cartridge(rom.into_boxed_slice())
        }
        pub fn title(&self) -> &str {
            from_utf8(&self.0[ROM_TITLE as usize..ROM_TITLE_END as usize]).unwrap()
        }
        pub fn size(&self) -> usize {
            if self[ROM_SIZE] < 0x50 {
                BANK_SIZE << (1 + self[ROM_SIZE])
            } else {
                match self[ROM_SIZE] {
                    0x52 => 72 * BANK_SIZE,
                    0x53 => 80 * BANK_SIZE,
                    0x54 => 96 * BANK_SIZE,
                    _inv => panic!("Invalid rom size {}", _inv),
                }
            }
        }
        pub fn num_banks(&self) -> usize {
            // utility, this is inferred from size, not stored directly
            if self.size() > BANK_SIZE * 2 {
                self.size() / BANK_SIZE
            } else {
                0
            }
        }
        pub fn size_ram(&self) -> usize {
            match self[ROM_RAM_SIZE] {
                0x00 => KB * 0,
                0x01 => KB * 2,
                0x02 => KB * 8,
                0x03 => KB * 32,
                0x04 => KB * 128,
                0x05 => KB * 64,
                _inv => panic!("Invalid RAM size {}", _inv),
            }
        }
        pub fn hardware_type(&self) -> &str {
            match self[ROM_TYPE] {
                0x00 => "ROM ONLY",
                0x01 => "MBC1",
                0x02 => "MBC1+RAM",
                0x03 => "MBC1+RAM+BATTERY",
                0x05 => "MBC2",
                0x06 => "MBC2+BATTERY",
                0x08 => "ROM+RAM",
                0x09 => "ROM+RAM+BATTERY",
                0x0B => "MMM01",
                0x0C => "MMM01+RAM",
                0x0D => "MMM01+RAM+BATTERY",
                0x0F => "MBC3+TIMER+BATTERY",
                0x10 => "MBC3+TIMER+RAM+BATTERY",
                0x11 => "MBC3",
                0x12 => "MBC3+RAM",
                0x13 => "MBC3+RAM+BATTERY",
                0x19 => "MBC5",
                0x1A => "MBC5+RAM",
                0x1B => "MBC5+RAM+BATTERY",
                0x1C => "MBC5+RUMBLE",
                0x1D => "MBC5+RUMBLE+RAM",
                0x1E => "MBC5+RUMBLE+RAM+BATTERY",
                0x20 => "MBC6",
                0x22 => "MBC7+SENSOR+RUMBLE+RAM+BATTERY",
                0xFC => "POCKET CAMERA",
                0xFD => "BANDAI TAMA5",
                0xFE => "HuC3",
                0xFF => "HuC1+RAM+BATTERY",
                _ => "???",
            }
        }
        pub fn destination_code(&self) -> &str {
            match self[ROM_DESTINATION] {
                0x00 => "Japanese",
                0x01 => "Non-Japanese",
                _ => "???",
            }
        }
    }
    impl Index<Word> for Cartridge {
        type Output = Byte;
        fn index(&self, index: Word) -> &Self::Output {
            &self.0[index as usize]
        }
    }
    impl Index<usize> for Cartridge {
        type Output = Byte;
        fn index(&self, index: usize) -> &Self::Output {
            &self.0[index]
        }
    }
    impl Index<std::ops::Range<usize>> for Cartridge {
        type Output = [Byte];
        fn index(&self, index: std::ops::Range<usize>) -> &Self::Output {
            &self.0[index]
        }
    }

    pub struct Memory {
        pub(crate) data: [Byte; MEM_SIZE],
        pub dma_req: bool,
    }
    impl Memory {
        pub fn new() -> Memory {
            let mut mem = Memory {
                data: [0; MEM_SIZE],
                dma_req: false,
            };
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
        pub fn load_rom(&mut self, cart: &Cartridge) {
            // raw copy, skip mem checks
            self.data[MEM_BANK_00 as usize..MEM_VRAM as usize]
                .copy_from_slice(&cart.0[MEM_BANK_00 as usize..MEM_VRAM as usize])
        }
        pub fn bank0(&mut self) -> &mut [Byte] {
            &mut self.data[MEM_BANK_00 as usize..MEM_BANK_NN as usize]
        }
        pub fn bank1(&mut self) -> &mut [Byte] {
            &mut self.data[MEM_BANK_NN as usize..MEM_VRAM as usize]
        }
        /// Update is called once per instruction decode
        ///
        /// todo: this shouldn't really be tied to the decode loop, the memory unit operates on its own little timeline
        pub fn update(&mut self) {
            if self.dma_req {
                self.dma_req = false;
                // todo: on real hardware this doesn't happen instantaneously, may need some code to delay the full transfer based on tsc
                // (e.g. while DMA is active the memory unit restricts access to everything but the HRAM)
                // https://gbdev.io/pandocs/OAM_DMA_Transfer.html#ff46--dma-oam-dma-source-address--start
                // Source:      $XX00-$XX9F   ;XX = $00 to $DF
                // Destination: $FE00-$FE9F
                let offset = self[DMA];
                let dma_start = crate::bits::combine(offset, 0x00) as usize;
                let dma_end = (crate::bits::combine(offset, 0x9F) + 1) as usize;
                let (main_chunk, oam_chunk) = self.data.split_at_mut(MEM_OAM as usize);
                oam_chunk[0..0xA0].copy_from_slice(&main_chunk[dma_start..dma_end]);
            }
        }
    }
    impl Index<Word> for Memory {
        type Output = Byte;
        fn index(&self, index: Word) -> &Self::Output {
            &self.data[index as usize]
        }
    }
    impl IndexMut<Word> for Memory {
        fn index_mut(&mut self, index: Word) -> &mut Self::Output {
            match index {
                DMA => {
                    println!("[write] DMA 0x{:X}", self[index]);
                    self.dma_req = true;
                }
                LCDC => println!("[write] LCDC"),
                _ => (),
            }
            &mut self.data[index as usize]
        }
    }
    impl Index<std::ops::Range<usize>> for Memory {
        type Output = [Byte];
        fn index(&self, index: std::ops::Range<usize>) -> &Self::Output {
            &self.data[index]
        }
    }
}

pub mod types {
    pub type Byte = u8;
    pub type Word = u16;
    pub type SByte = i8;
    pub type SWord = i16;

    // indices
    pub const REG_B: usize = 0;
    pub const REG_C: usize = 1;
    pub const REG_D: usize = 2;
    pub const REG_E: usize = 3;
    pub const REG_H: usize = 4;
    pub const REG_L: usize = 5;
    pub const FLAGS: usize = 6;
    pub const REG_A: usize = 7;

    // cpu flags
    pub const FL_Z: Byte = 1 << 7;
    pub const FL_N: Byte = 1 << 6;
    pub const FL_H: Byte = 1 << 5;
    pub const FL_C: Byte = 1 << 4;

    // bit masks
    pub const BIT_0: Byte = 1 << 0;
    pub const BIT_1: Byte = 1 << 1;
    pub const BIT_2: Byte = 1 << 2;
    pub const BIT_3: Byte = 1 << 3;
    pub const BIT_4: Byte = 1 << 4;
    pub const BIT_5: Byte = 1 << 5;
    pub const BIT_6: Byte = 1 << 6;
    pub const BIT_7: Byte = 1 << 7;

    #[derive(PartialEq, Debug)]
    pub struct Instruction {
        pub mnm: String,
        pub len: u8, // bytes to read
    }
    #[derive(PartialEq, Debug)]
    pub struct InstructionCB {
        pub opcode: &'static str,
        pub bit: u8,
        pub reg: usize,
    }
    impl Instruction {
        pub fn new(text: &str, len: u8) -> Self {
            Self {
                mnm: String::from(text),
                len,
            }
        }

        pub fn from_cb(icb: &InstructionCB) -> Self {
            Self {
                mnm: if icb.bit < 0xff {
                    format!("{} {}, {}", icb.opcode, icb.bit, crate::decode::R[icb.reg])
                } else {
                    format!("{}, {}", icb.opcode, crate::decode::R[icb.reg])
                },
                len: 1,
            }
        }

        pub fn valid(&self) -> bool {
            self.len > 0
        }

        pub fn prefix(&self) -> bool {
            self.mnm == crate::decode::CBPREFIX
        }

        pub fn mnm_args(&self, rom: &[Byte]) -> String {
            match rom.len() {
                1 => self.mnm.replace('n', &format!("${:02x}", rom[0])),
                2 => self.mnm.replace(
                    "nn",
                    &format!("${:04x}", crate::bits::combine(rom[1], rom[0])),
                ),
                _ => panic!("mnemonic only intended for instructions with args"),
            }
        }
    }
}

pub mod decode {
    use crate::types::*;

    // https://gb-archive.github.io/salvage/decoding_gbz80_opcodes/Decoding%20Gamboy%20Z80%20Opcodes.html
    // https://www.pastraiser.com/cpu/gameboy/gameboy_opcodes.html

    // used for CB decoding, some bit functions reference (HL) instead of a register
    pub const ADR_HL: usize = 6;
    const R_ID: [usize; 8] = [REG_B, REG_C, REG_D, REG_E, REG_H, REG_L, ADR_HL, REG_A];

    // arg tables for printing mnemonics
    pub const R: [&'static str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
    const RP: [&'static str; 4] = ["BC", "DE", "HL", "SP"];
    const RP2: [&'static str; 4] = ["BC", "DE", "HL", "AF"];
    const CC: [&'static str; 4] = ["NZ", "Z", "NC", "C"];
    const ALU: [&'static str; 8] = [
        "ADD A,", "ADC A,", "SUB", "SBC A,", "AND", "XOR", "OR", "CP",
    ];
    const ROT: [&'static str; 8] = ["RLC", "RRC", "RL", "RR", "SLA", "SRA", "SWAP", "SRL"];

    // """
    // Upon establishing the opcode, the Z80's path of action is generally dictated by these values:

    // x = the opcode's 1st octal digit (i.e. bits 7-6)
    // y = the opcode's 2nd octal digit (i.e. bits 5-3)
    // z = the opcode's 3rd octal digit (i.e. bits 2-0)
    // p = y rightshifted one position (i.e. bits 5-4)
    // q = y modulo 2 (i.e. bit 3)

    // The following placeholders for instructions and operands are used:

    // d = displacement byte (8-bit signed integer)
    // n = 8-bit immediate operand (unsigned integer)
    // nn = 16-bit immediate operand (unsigned integer)
    // tab[x] = whatever is contained in the table named tab at index x (analogous for y and z and other table names)
    // """

    const fn x(op: Byte) -> Byte {
        op >> 6
    }
    const fn y(op: Byte) -> Byte {
        op >> 3 & 0b111
    }
    const fn z(op: Byte) -> Byte {
        op & 0b111
    }
    const fn p(op: Byte) -> Byte {
        y(op) >> 1
    }
    const fn q(op: Byte) -> Byte {
        y(op) & 0b1
    }

    const INVALID: &'static str = "INVALID";
    pub const CBPREFIX: &'static str = "(CB PREFIX)";

    // todo: Instruction is constantly allocating heap strings, I feel like there
    // should be a way to do this at compile time but I can't figure it out
    #[allow(non_snake_case)]
    pub fn decode(op: Byte) -> Instruction {
        let _ALU_y = ALU[y(op) as usize % ALU.len()];
        let _CC_y = CC[y(op) as usize % CC.len()];
        let _R_y = R[y(op) as usize % R.len()];
        let _R_z = R[z(op) as usize % R.len()];
        let _RP_p = RP[p(op) as usize % RP.len()];
        let _RP2_p = RP2[p(op) as usize % RP2.len()];
        let _y8 = y(op) * 8;
        match x(op) {
            0 => match z(op) {
                0 => match y(op) {
                    0 => Instruction::new("NOP", 1),
                    1 => Instruction::new("LD (nn), SP", 3),
                    2 => Instruction::new("STOP", 1),
                    3 => Instruction::new("JR n", 2),
                    v @ 4..=7 => {
                        let i: usize = (v - 4) as usize;
                        let _CC_i: &'static str = CC[i];
                        Instruction {
                            mnm: format!("JR {_CC_i}, n"),
                            len: 2,
                        }
                    }
                    _ => Instruction::new(INVALID, 0),
                },
                1 => match q(op) {
                    0 => Instruction {
                        mnm: format!("LD {_RP_p}, nn"),
                        len: 3,
                    },
                    1 => Instruction {
                        mnm: format!("ADD HL, {_RP_p}"),
                        len: 1,
                    },
                    _ => Instruction::new(INVALID, 0),
                },
                2 => match q(op) {
                    0 => match p(op) {
                        0 => Instruction::new("LD (BC), A", 1),
                        1 => Instruction::new("LD (DE), A", 1),
                        2 => Instruction::new("LD (HL+), A", 1),
                        3 => Instruction::new("LD (HL-), A", 1),
                        _ => Instruction::new(INVALID, 0),
                    },
                    1 => match p(op) {
                        0 => Instruction::new("LD A, (BC)", 1),
                        1 => Instruction::new("LD A, (DE)", 1),
                        2 => Instruction::new("LD A, (HL+)", 1),
                        3 => Instruction::new("LD A, (HL-)", 1),
                        _ => Instruction::new(INVALID, 0),
                    },
                    _ => Instruction::new(INVALID, 0),
                },
                3 => match q(op) {
                    0 => Instruction {
                        mnm: format!("INC {_RP_p}"),
                        len: 1,
                    },
                    1 => Instruction {
                        mnm: format!("DEC, {_RP_p}"),
                        len: 1,
                    },
                    _ => Instruction::new(INVALID, 0),
                },
                4 => Instruction {
                    mnm: format!("INC {_R_y}"),
                    len: 1,
                },
                5 => Instruction {
                    mnm: format!("DEC {_R_y}"),
                    len: 1,
                },
                6 => Instruction {
                    mnm: format!("LD {_R_y}, n"),
                    len: 2,
                },
                7 => match y(op) {
                    0 => Instruction::new("RLCA", 1),
                    1 => Instruction::new("RRCA", 1),
                    2 => Instruction::new("RLA", 1),
                    3 => Instruction::new("RRA", 1),
                    4 => Instruction::new("DAA", 1),
                    5 => Instruction::new("CPL", 1),
                    6 => Instruction::new("SCF", 1),
                    7 => Instruction::new("CCF", 1),
                    _ => Instruction::new(INVALID, 0),
                },
                _ => Instruction::new(INVALID, 0),
            },
            1 => match z(op) {
                6 => match y(op) {
                    6 => Instruction::new("HALT", 1),
                    _ => Instruction::new(INVALID, 0),
                },
                _ => Instruction {
                    mnm: format!("LD {_R_y}, {_R_z}"),
                    len: 1,
                },
            },
            2 => Instruction {
                mnm: format!("{_ALU_y} {_R_z}"),
                len: 1,
            },
            3 => match z(op) {
                0 => match y(op) {
                    0..=3 => Instruction {
                        mnm: format!("RET {_CC_y}"),
                        len: 1,
                    },
                    4 => Instruction::new("LD (0xFF00 + n), A", 2),
                    5 => Instruction::new("ADD SP, n", 2),
                    6 => Instruction::new("LD A, (0xFF00 + n)", 2),
                    7 => Instruction::new("LD HL, SP + n", 2),
                    _ => Instruction::new(INVALID, 0),
                },
                1 => match q(op) {
                    0 => Instruction {
                        mnm: format!("POP {_RP2_p}"),
                        len: 1,
                    },
                    1 => match p(op) {
                        0 => Instruction::new("RET", 1),
                        1 => Instruction::new("RETI", 1),
                        2 => Instruction::new("JP HL", 1),
                        3 => Instruction::new("LD SP, HL", 1),
                        _ => Instruction::new(INVALID, 0),
                    },
                    _ => Instruction::new(INVALID, 0),
                },
                2 => match y(op) {
                    0..=3 => Instruction {
                        mnm: format!("JP {_CC_y}, nn"),
                        len: 3,
                    },
                    4 => Instruction::new("LD (0xFF00 + C), A", 1),
                    5 => Instruction::new("LD (nn), A", 3),
                    6 => Instruction::new("LD A, (0xFF00 + C)", 1),
                    7 => Instruction::new("LD A, (nn)", 3),
                    _ => Instruction::new(INVALID, 0),
                },
                3 => match y(op) {
                    0 => Instruction::new("JP nn", 3),
                    1 => Instruction::new(CBPREFIX, 1),
                    6 => Instruction::new("DI", 1),
                    7 => Instruction::new("EI", 1),
                    _ => Instruction::new(INVALID, 0),
                },
                4 => match y(op) {
                    0..=3 => Instruction {
                        mnm: format!("CALL {_CC_y}, nn"),
                        len: 3,
                    },
                    _ => Instruction::new(INVALID, 0),
                },
                5 => match q(op) {
                    0 => Instruction {
                        mnm: format!("PUSH {_RP2_p}"),
                        len: 1,
                    },
                    1 => match p(op) {
                        0 => Instruction::new("CALL nn", 3),
                        _ => Instruction::new(INVALID, 0),
                    },
                    _ => Instruction::new(INVALID, 0),
                },
                6 => Instruction {
                    mnm: format!("{_ALU_y} n"),
                    len: 2,
                },
                7 => Instruction {
                    mnm: format!("RST {_y8:02x}H"),
                    len: 1,
                },
                _ => todo!(),
            },
            _ => todo!(),
        }
    }

    #[allow(non_snake_case)]
    pub fn decodeCB(op: Byte) -> InstructionCB {
        let _ROT_y = ROT[y(op) as usize];
        let _R_z = R_ID[z(op) as usize];
        let _y = y(op);
        match x(op) {
            0 => InstructionCB {
                // mnm: format!("{_ROT_y} {_R_z}"),
                opcode: _ROT_y,
                bit: 0xFF,
                reg: _R_z,
            },
            1 => InstructionCB {
                // mnm: format!("BIT {_y}, {_R_z}"),
                opcode: "BIT",
                bit: _y,
                reg: _R_z,
            },
            2 => InstructionCB {
                // mnm: format!("RES {_y}, {_R_z}"),
                opcode: "RES",
                bit: _y,
                reg: _R_z,
            },
            3 => InstructionCB {
                // mnm: format!("SET {_y}, {_R_z}"),
                opcode: "SET",
                bit: _y,
                reg: _R_z,
            },
            _ => InstructionCB {
                opcode: "INVALID",
                bit: 0xFF,
                reg: usize::max_value(),
            },
        }
    }

    #[cfg(test)]
    mod tests_decode {
        use super::*;
        #[test]
        fn test_xyzpq() {
            let t = 0b11_010_001;
            assert_eq!(x(t), 0b11);
            assert_eq!(y(t), 0b010);
            assert_eq!(z(t), 0b001);
            assert_eq!(p(t), 0b01);
            assert_eq!(q(t), 0b0);
        }

        #[test]
        #[rustfmt::skip]
        fn test_reg_b() {
            assert_eq!(decodeCB(0x00), InstructionCB{opcode:"RLC",  bit: 0xff, reg: REG_B});
            assert_eq!(decodeCB(0x10), InstructionCB{opcode:"RL",   bit: 0xff, reg: REG_B});
            assert_eq!(decodeCB(0x20), InstructionCB{opcode:"SLA",  bit: 0xff, reg: REG_B});
            assert_eq!(decodeCB(0x30), InstructionCB{opcode:"SWAP", bit: 0xff, reg: REG_B});
            assert_eq!(decodeCB(0x40), InstructionCB{opcode:"BIT",  bit: 0,    reg: REG_B});
            assert_eq!(decodeCB(0x50), InstructionCB{opcode:"BIT",  bit: 2,    reg: REG_B});
            assert_eq!(decodeCB(0x60), InstructionCB{opcode:"BIT",  bit: 4,    reg: REG_B});
            assert_eq!(decodeCB(0x70), InstructionCB{opcode:"BIT",  bit: 6,    reg: REG_B});
            assert_eq!(decodeCB(0x80), InstructionCB{opcode:"RES",  bit: 0,    reg: REG_B});
            assert_eq!(decodeCB(0x90), InstructionCB{opcode:"RES",  bit: 2,    reg: REG_B});
            assert_eq!(decodeCB(0xA0), InstructionCB{opcode:"RES",  bit: 4,    reg: REG_B});
            assert_eq!(decodeCB(0xB0), InstructionCB{opcode:"RES",  bit: 6,    reg: REG_B});
            assert_eq!(decodeCB(0xC0), InstructionCB{opcode:"SET",  bit: 0,    reg: REG_B});
            assert_eq!(decodeCB(0xD0), InstructionCB{opcode:"SET",  bit: 2,    reg: REG_B});
            assert_eq!(decodeCB(0xE0), InstructionCB{opcode:"SET",  bit: 4,    reg: REG_B});
            assert_eq!(decodeCB(0xF0), InstructionCB{opcode:"SET",  bit: 6,    reg: REG_B});
            
            assert_eq!(decodeCB(0x08), InstructionCB{opcode:"RRC",  bit: 0xff, reg: REG_B});
            assert_eq!(decodeCB(0x18), InstructionCB{opcode:"RR",   bit: 0xff, reg: REG_B});
            assert_eq!(decodeCB(0x28), InstructionCB{opcode:"SRA",  bit: 0xff, reg: REG_B});
            assert_eq!(decodeCB(0x38), InstructionCB{opcode:"SRL",  bit: 0xff, reg: REG_B});
            assert_eq!(decodeCB(0x48), InstructionCB{opcode:"BIT",  bit: 1,    reg: REG_B});
            assert_eq!(decodeCB(0x58), InstructionCB{opcode:"BIT",  bit: 3,    reg: REG_B});
            assert_eq!(decodeCB(0x68), InstructionCB{opcode:"BIT",  bit: 5,    reg: REG_B});
            assert_eq!(decodeCB(0x78), InstructionCB{opcode:"BIT",  bit: 7,    reg: REG_B});
            assert_eq!(decodeCB(0x88), InstructionCB{opcode:"RES",  bit: 1,    reg: REG_B});
            assert_eq!(decodeCB(0x98), InstructionCB{opcode:"RES",  bit: 3,    reg: REG_B});
            assert_eq!(decodeCB(0xA8), InstructionCB{opcode:"RES",  bit: 5,    reg: REG_B});
            assert_eq!(decodeCB(0xB8), InstructionCB{opcode:"RES",  bit: 7,    reg: REG_B});
            assert_eq!(decodeCB(0xC8), InstructionCB{opcode:"SET",  bit: 1,    reg: REG_B});
            assert_eq!(decodeCB(0xD8), InstructionCB{opcode:"SET",  bit: 3,    reg: REG_B});
            assert_eq!(decodeCB(0xE8), InstructionCB{opcode:"SET",  bit: 5,    reg: REG_B});
            assert_eq!(decodeCB(0xF8), InstructionCB{opcode:"SET",  bit: 7,    reg: REG_B});
        }
    }
}

pub mod io {
    use crate::types::Byte;
    use std::io::Read;

    pub fn read_bytes(path: &str) -> Vec<Byte> {
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(file) => panic!("failed to open {}", file),
        };
        let info = file.metadata().expect("failed to read file info");

        // todo: not sure if I actually want this but it made clippy happy
        // consider instead #[allow(clippy::unused_io_amount)]
        let mut rom: Vec<Byte> = vec![0; info.len() as usize];
        file.read_exact(&mut rom)
            .expect("failed to read file into memory");

        rom
    }
}

pub mod bits {
    use crate::types::{Byte, SByte, Word};

    pub const HIGH_MASK: Word = 0xFF00;
    pub const LOW_MASK: Word = 0x00FF;
    pub const HIGH_MASK_NIB: Byte = 0xF0;
    pub const LOW_MASK_NIB: Byte = 0x0F;

    pub const fn hi(reg: Word) -> Byte {
        (reg >> Byte::BITS) as Byte
    }

    pub const fn lo(reg: Word) -> Byte {
        (reg & LOW_MASK) as Byte
    }

    pub const fn combine(high: Byte, low: Byte) -> Word {
        (high as Word) << Byte::BITS | (low as Word)
    }

    pub const fn fl_z(val: Byte) -> Byte {
        if val == 0 {
            crate::types::FL_Z
        } else {
            0
        }
    }

    pub const fn bit(idx: Byte, val: Byte) -> Byte {
        (val >> idx) & 1
    }

    pub const fn bit_test(idx: Byte, val: Byte) -> bool {
        bit(idx, val) != 0
    }

    #[test]
    fn test_bit_test() {
        let x: Byte = 0b00000101;
        assert_eq!(bit_test(7, x), false);
        assert_eq!(bit_test(6, x), false);
        assert_eq!(bit_test(5, x), false);
        assert_eq!(bit_test(4, x), false);
        assert_eq!(bit_test(3, x), false);
        assert_eq!(bit_test(2, x), true);
        assert_eq!(bit_test(1, x), false);
        assert_eq!(bit_test(0, x), true);
    }

    // can't be const for some reason https://github.com/rust-lang/rust/issues/53605
    pub fn signed(val: Byte) -> SByte {
        unsafe { std::mem::transmute(val) }
    }
}

pub mod dbg {
    use std::io::Write;
    use std::fs;

    use crate::cpu::*;
    use crate::memory::*;
    use crate::types::*;

    pub fn log_cpu(path: &str, cpu: &CPUState, mem: &Memory) -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        write!(file,"A:{:02X} F:{:02X} B:{:02X} C:{:02X} D:{:02X} E:{:02X} H:{:02X} L:{:02X} SP:{:04X} PC:{:04X} PCMEM:{:02X},{:02X},{:02X},{:02X}\n",
        cpu.reg[REG_A],
        cpu.reg[FLAGS],
        cpu.reg[REG_B],
        cpu.reg[REG_C],
        cpu.reg[REG_D],
        cpu.reg[REG_E],
        cpu.reg[REG_H],
        cpu.reg[REG_L],
        cpu.sp,
        cpu.pc,
        mem[cpu.pc],
        mem[cpu.pc+1],
        mem[cpu.pc+2],
        mem[cpu.pc+3],
        )?;
        Ok(())
    }

    pub fn dump(path: &str, mem: &Memory) -> std::io::Result<()> {
        fs::write(path, mem.data)?;
        Ok(())
    }
}
