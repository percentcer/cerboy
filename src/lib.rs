#![allow(non_snake_case)]
#![allow(dead_code)]

pub mod cpu {
    use crate::bits::*;
    use crate::decode::*;
    use crate::memory::*;
    use crate::types::*;

    // https://gbdev.gg8.se/files/docs/mirrors/pandocs.html
    // https://rgbds.gbdev.io/docs/v0.7.0/gbz80.7
    // https://gbdev.io/pandocs/CPU_Instruction_Set.html
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
    pub const TICKS_PER_FRAME: u64 =
        (TICKS_PER_SCANLINE * GB_SCREEN_HEIGHT as u64) + TICKS_PER_VBLANK; // 70224 cycles

    pub const TICKS_PER_DIV_INC: u64 = 256;

    // tile constants
    pub const BYTES_PER_TILE: u16 = 16;

    // interrupt flags
    pub const FL_INT_VBLANK: Byte = 1 << 0;
    pub const FL_INT_STAT: Byte = 1 << 1;
    pub const FL_INT_TIMER: Byte = 1 << 2;
    pub const FL_INT_SERIAL: Byte = 1 << 3;
    pub const FL_INT_JOYPAD: Byte = 1 << 4;

    #[derive(Debug, Clone)]
    pub struct UnknownInstructionError {
        mnm: String,
        op: Byte,
    }

    impl std::fmt::Display for UnknownInstructionError {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "unknown instruction 0x{:X} ({})", self.op, self.mnm)
        }
    }

    #[derive(Copy, Clone, Debug)]
    pub struct CPUState {
        // ------------ meta, not part of actual gb hardware but useful
        pub tsc: u64,        // counting cycles since reset
        pub inst_count: u64, // counting instructions since reset
        pub inst_ei: u64, // timestamp when ei was set, used to keep track of the two-instruction-delay
        // ------------ hardware
        pub reg: [Byte; 8],
        pub sp: Word,
        pub pc: Word,
        pub ime: bool,  // true == interrupts enabled
        pub halt: bool, // true == don't execute anything until interrupt
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
                halt: false,
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

    pub fn next(cpu: CPUState, mem: &mut Memory) -> Result<CPUState, UnknownInstructionError> {
        // fetch and execute
        // -----------------
        let pc = cpu.pc;
        let cpu = CPUState {
            inst_count: cpu.inst_count + 1,
            ..cpu
        }; // referenced by interrupt enabling instructions
        let op = mem.read(pc);
        // cerboy::decode::print_op(op);

        // todo; inst count is not the same as tick, halt state makes this above incorrect

        // check interrupts
        // -----------------
        // https://gbdev.io/pandocs/single.html#ime-interrupt-master-enable-flag-write-only
        // The effect of EI is delayed by one instruction.
        // This means that EI followed immediately by DI does not
        // allow interrupts between the EI and the DI.
        let ei_valid_delay = (cpu.inst_count - cpu.inst_ei) > 1;
        let enabled_flags = mem.read(IE) & mem.read(IF);

        // possibly unhalt the cpu
        let cpu = if enabled_flags != 0 {
            CPUState { halt: false, ..cpu }
        } else {
            cpu
        };

        if cpu.ime && ei_valid_delay && enabled_flags != 0 {
            if (enabled_flags & FL_INT_VBLANK) != 0 {
                Ok(jump_to_int_vec(cpu, mem, FL_INT_VBLANK, VEC_INT_VBLANK))
            } else if (enabled_flags & FL_INT_STAT) != 0 {
                Ok(jump_to_int_vec(cpu, mem, FL_INT_STAT, VEC_INT_STAT))
            } else if (enabled_flags & FL_INT_TIMER) != 0 {
                Ok(jump_to_int_vec(cpu, mem, FL_INT_TIMER, VEC_INT_TIMER))
            } else if (enabled_flags & FL_INT_SERIAL) != 0 {
                Ok(jump_to_int_vec(cpu, mem, FL_INT_SERIAL, VEC_INT_SERIAL))
            } else if (enabled_flags & FL_INT_JOYPAD) != 0 {
                Ok(jump_to_int_vec(cpu, mem, FL_INT_JOYPAD, VEC_INT_JOYPAD))
            } else {
                panic!("interrupt enabled but unknown flag?")
            }
        } else if cpu.halt {
            // halted, just pass the time
            Ok(cpu.tick(4))
        } else {
            // todo: is this correct? I'm assuming it can't handle an interrupt
            // and then go right into the next instruction, it's one or the other
            let inst = crate::decode::decode(op);
            match op {
                0x00 => Ok(nop(cpu)),
                0x01 => Ok(ld_bc_d16(cpu, mem.read(pc + 1), mem.read(pc + 2))),
                0x02 => Ok(ld_BC_a(cpu, mem)),
                0x03 => Ok(inc_bc(cpu)),
                0x04 => Ok(inc_b(cpu)),
                0x05 => Ok(dec_b(cpu)),
                0x06 => Ok(ld_b_d8(cpu, mem.read(pc + 1))),
                0x07 => Ok(rlca(cpu)),
                0x08 => Ok(ld_A16_sp(mem.read(pc + 1), mem.read(pc + 2), cpu, mem)),
                0x09 => Ok(add_hl_bc(cpu)),
                0x0A => Ok(ld_a_BC(cpu, &mem)),
                0x0B => Ok(dec_bc(cpu)),
                0x0C => Ok(inc_c(cpu)),
                0x0D => Ok(dec_c(cpu)),
                0x0E => Ok(ld_c_d8(cpu, mem.read(pc + 1))),
                0x0F => Ok(rrca(cpu)),
                0x10 => Ok(stop(cpu)),
                0x11 => Ok(ld_de_d16(cpu, mem.read(pc + 1), mem.read(pc + 2))),
                0x12 => Ok(ld_DE_a(cpu, mem)),
                0x13 => Ok(inc_de(cpu)),
                0x14 => Ok(inc_d(cpu)),
                0x15 => Ok(dec_d(cpu)),
                0x16 => Ok(ld_d_d8(cpu, mem.read(pc + 1))),
                0x17 => Ok(rla(cpu)),
                0x18 => Ok(jr_r8(cpu, signed(mem.read(pc + 1)))),
                0x19 => Ok(add_hl_de(cpu)),
                0x1A => Ok(ld_a_DE(cpu, &mem)),
                0x1B => Ok(dec_de(cpu)),
                0x1C => Ok(inc_e(cpu)),
                0x1D => Ok(dec_e(cpu)),
                0x1E => Ok(ld_e_d8(cpu, mem.read(pc + 1))),
                0x1F => Ok(rra(cpu)),
                0x20 => Ok(jr_nz_r8(cpu, signed(mem.read(pc + 1)))),
                0x21 => Ok(ld_hl_d16(cpu, mem.read(pc + 1), mem.read(pc + 2))),
                0x22 => Ok(ldi_HL_a(cpu, mem)),
                0x23 => Ok(inc_hl(cpu)),
                0x24 => Ok(inc_h(cpu)),
                0x25 => Ok(dec_h(cpu)),
                0x26 => Ok(ld_h_d8(cpu, mem.read(pc + 1))),
                0x27 => Ok(daa(cpu)),
                0x28 => Ok(jr_z_r8(cpu, signed(mem.read(pc + 1)))),
                0x29 => Ok(add_hl_hl(cpu)),
                0x2A => Ok(ldi_a_HL(cpu, mem)),
                0x2B => Ok(dec_hl(cpu)),
                0x2C => Ok(inc_l(cpu)),
                0x2D => Ok(dec_l(cpu)),
                0x2E => Ok(ld_l_d8(cpu, mem.read(pc + 1))),
                0x2F => Ok(cpl(cpu)),
                0x30 => Ok(jr_nc_r8(cpu, signed(mem.read(pc + 1)))),
                0x31 => Ok(ld_sp_d16(cpu, mem.read(pc + 1), mem.read(pc + 2))),
                0x32 => Ok(ldd_HL_a(cpu, mem)),
                0x33 => Ok(inc_sp(cpu)),
                0x34 => Ok(inc_HL(cpu, mem)),
                0x35 => Ok(dec_HL(cpu, mem)),
                0x36 => Ok(ld_HL_d8(cpu, mem.read(pc + 1), mem)),
                0x37 => Ok(scf(cpu)),
                0x38 => Ok(jr_c_r8(cpu, signed(mem.read(pc + 1)))),
                0x39 => Ok(add_hl_sp(cpu)),
                0x3A => Ok(ldd_a_HL(cpu, mem)),
                0x3B => Ok(dec_sp(cpu)),
                0x3C => Ok(inc_a(cpu)),
                0x3D => Ok(dec_a(cpu)),
                0x3E => Ok(ld_a_d8(cpu, mem.read(pc + 1))),
                0x3F => Ok(ccf(cpu)),
                0x40..=0x7F => match op {
                    0x46 => Ok(ld_b_HL(cpu, &mem)),
                    0x4E => Ok(ld_c_HL(cpu, &mem)),
                    0x56 => Ok(ld_d_HL(cpu, &mem)),
                    0x5E => Ok(ld_e_HL(cpu, &mem)),
                    0x66 => Ok(ld_h_HL(cpu, &mem)),
                    0x6E => Ok(ld_l_HL(cpu, &mem)),
                    0x76 => Ok(halt(cpu)),
                    0x7E => Ok(ld_a_HL(cpu, &mem)),
                    0x70 => Ok(ld_HL_b(cpu, mem)),
                    0x71 => Ok(ld_HL_c(cpu, mem)),
                    0x72 => Ok(ld_HL_d(cpu, mem)),
                    0x73 => Ok(ld_HL_e(cpu, mem)),
                    0x74 => Ok(ld_HL_h(cpu, mem)),
                    0x75 => Ok(ld_HL_l(cpu, mem)),
                    0x77 => Ok(ld_HL_a(cpu, mem)),
                    _ => Ok(ld_r_r(cpu, op)),
                },
                0x80..=0xBF => {
                    let fn_r = [add_r, adc_r, sub_r, sbc_r, and_r, xor_r, or_r, cp_r];
                    let fn_HL = [add_HL, adc_HL, sub_HL, sbc_HL, and_HL, xor_HL, or_HL, cp_HL];

                    let src_idx = (op % 8) as usize;
                    let fn_idx = ((op - 0x80) / 8) as usize;

                    let src = R_ID[src_idx];
                    if src != ADR_HL {
                        Ok(fn_r[fn_idx](cpu, src))
                    } else {
                        Ok(fn_HL[fn_idx](cpu, mem))
                    }
                }
                0xC0 => Ok(ret_nz(cpu, &mem)),
                0xC1 => Ok(pop_bc(cpu, &mem)),
                0xC2 => Ok(jp_f_d16(cpu, mem.read(pc + 1), mem.read(pc + 2), 0xC2)),
                0xC3 => Ok(jp_d16(cpu, mem.read(pc + 1), mem.read(pc + 2))),
                0xC4 => Ok(call_f_d16(
                    mem.read(pc + 1),
                    mem.read(pc + 2),
                    cpu,
                    mem,
                    0xC4,
                )),
                0xC5 => Ok(push_bc(cpu, mem)),
                0xC6 => Ok(add_d8(cpu, mem.read(pc + 1))),
                0xC7 => Ok(rst_n(cpu, mem, 0xC7)),
                0xC8 => Ok(ret_z(cpu, &mem)),
                0xC9 => Ok(ret(cpu, &mem)),
                0xCA => Ok(jp_f_d16(cpu, mem.read(pc + 1), mem.read(pc + 2), 0xCA)),
                0xCB => {
                    let op_cb = mem.read(pc + 1);
                    let icb = decodeCB(op_cb);
                    if icb.reg == ADR_HL {
                        match icb.opcode {
                            "RLC" => Ok(rlc_hl(cpu, mem)),
                            "RRC" => Ok(rrc_hl(cpu, mem)),
                            "RL" => Ok(rl_hl(cpu, mem)),
                            "RR" => Ok(rr_hl(cpu, mem)),
                            "SLA" => Ok(sla_hl(cpu, mem)),
                            "SRA" => Ok(sra_hl(cpu, mem)),
                            "SWAP" => Ok(swap_hl(cpu, mem)),
                            "SRL" => Ok(srl_hl(cpu, mem)),
                            "BIT" => Ok(bit_hl(cpu, mem, icb.bit)),
                            "RES" => Ok(res_n_hl(cpu, mem, icb.bit)),
                            "SET" => Ok(set_hl(cpu, mem, icb.bit)),
                            _ => panic!("0xCB (HL) unknown instruction, should be unreachable!"),
                        }
                    } else {
                        match icb.opcode {
                            "RLC" => Ok(rlc_r(cpu, icb.reg)),
                            "RRC" => Ok(rrc_r(cpu, icb.reg)),
                            "RL" => Ok(rl_r(cpu, icb.reg)),
                            "RR" => Ok(rr_r(cpu, icb.reg)),
                            "SLA" => Ok(sla_r(cpu, icb.reg)),
                            "SRA" => Ok(sra_r(cpu, icb.reg)),
                            "SWAP" => Ok(swap_r(cpu, icb.reg)),
                            "SRL" => Ok(srl_r(cpu, icb.reg)),
                            "BIT" => Ok(bit_r(cpu, icb.bit, icb.reg)),
                            "RES" => Ok(res_n_r(cpu, icb.bit, icb.reg)),
                            "SET" => Ok(set_r(cpu, icb.bit, icb.reg)),
                            _ => panic!("0xCB (reg) unknown instruction, should be unreachable!"),
                        }
                    }
                }
                0xCC => Ok(call_f_d16(
                    mem.read(pc + 1),
                    mem.read(pc + 2),
                    cpu,
                    mem,
                    0xCC,
                )),
                0xCD => Ok(call_d16(mem.read(pc + 1), mem.read(pc + 2), cpu, mem)),
                0xCE => Ok(adc_d8(cpu, mem.read(pc + 1))),
                0xCF => Ok(rst_n(cpu, mem, 0xCF)),
                0xD0 => Ok(ret_nc(cpu, &mem)),
                0xD1 => Ok(pop_de(cpu, &mem)),
                0xD2 => Ok(jp_f_d16(cpu, mem.read(pc + 1), mem.read(pc + 2), 0xD2)),
                0xD3 => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xD4 => Ok(call_f_d16(
                    mem.read(pc + 1),
                    mem.read(pc + 2),
                    cpu,
                    mem,
                    0xD4,
                )),
                0xD5 => Ok(push_de(cpu, mem)),
                0xD6 => Ok(sub_d8(cpu, mem.read(pc + 1))),
                0xD7 => Ok(rst_n(cpu, mem, 0xD7)),
                0xD8 => Ok(ret_c(cpu, &mem)),
                0xD9 => Ok(reti(cpu, &mem)),
                0xDA => Ok(jp_f_d16(cpu, mem.read(pc + 1), mem.read(pc + 2), 0xDA)),
                0xDB => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xDC => Ok(call_f_d16(
                    mem.read(pc + 1),
                    mem.read(pc + 2),
                    cpu,
                    mem,
                    0xDC,
                )),
                0xDD => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xDE => Ok(sbc_d8(cpu, mem.read(pc + 1))),
                0xDF => Ok(rst_n(cpu, mem, 0xDF)),
                0xE0 => Ok(ld_FF00_A8_a(mem.read(pc + 1), cpu, mem)),
                0xE1 => Ok(pop_hl(cpu, &mem)),
                0xE2 => Ok(ld_FF00_C_a(cpu, mem)),
                0xE3 => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xE4 => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xE5 => Ok(push_hl(cpu, mem)),
                0xE6 => Ok(and_d8(cpu, mem.read(pc + 1))),
                0xE7 => Ok(rst_n(cpu, mem, 0xE7)),
                0xE8 => Ok(add_sp_r8(cpu, signed(mem.read(pc + 1)))),
                0xE9 => Ok(jp_hl(cpu)),
                0xEA => Ok(ld_A16_a(mem.read(pc + 1), mem.read(pc + 2), cpu, mem)),
                0xEB => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xEC => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xED => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xEE => Ok(xor_d8(cpu, mem.read(pc + 1))),
                0xEF => Ok(rst_n(cpu, mem, 0xEF)),
                0xF0 => Ok(ld_a_FF00_A8(cpu, &mem, mem.read(pc + 1))),
                0xF1 => Ok(pop_af(cpu, &mem)),
                0xF2 => Ok(ld_a_FF00_C(cpu, &mem)),
                0xF3 => Ok(di(cpu)),
                0xF4 => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xF5 => Ok(push_af(cpu, mem)),
                0xF6 => Ok(or_d8(cpu, mem.read(pc + 1))),
                0xF7 => Ok(rst_n(cpu, mem, 0xF7)),
                0xF8 => Ok(ld_hl_sp_r8(cpu, signed(mem.read(pc + 1)))),
                0xF9 => Ok(ld_sp_hl(cpu)),
                0xFA => Ok(ld_a_A16(mem.read(pc + 1), mem.read(pc + 2), cpu, &mem)),
                0xFB => Ok(ei(cpu)),
                0xFC => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xFD => Err(UnknownInstructionError { op, mnm: inst.mnm }),
                0xFE => Ok(cp_d8(cpu, mem.read(pc + 1))),
                0xFF => Ok(rst_n(cpu, mem, 0xFF)),
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
        mem.write(cpu.HL(), val);
        CPUState { ..cpu }
    }

    //   ld   r,r         xx         4 ---- r=r
    // ----------------------------------------------------------------------------
    const fn ld_r_r(cpu: CPUState, opcode: Byte) -> CPUState {
        let dst_idx = (opcode - 0x40) / 0x08;
        let src_idx = opcode % 0x08;
        impl_ld_r_d8(cpu, R_ID[dst_idx as usize], cpu.reg[R_ID[src_idx as usize]])
            .adv_pc(1)
            .tick(4)
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
        impl_ld_r_d8(cpu, REG_B, mem.read(cpu.HL()))
            .adv_pc(1)
            .tick(8)
    }
    fn ld_c_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_ld_r_d8(cpu, REG_C, mem.read(cpu.HL()))
            .adv_pc(1)
            .tick(8)
    }
    fn ld_d_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_ld_r_d8(cpu, REG_D, mem.read(cpu.HL()))
            .adv_pc(1)
            .tick(8)
    }
    fn ld_e_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_ld_r_d8(cpu, REG_E, mem.read(cpu.HL()))
            .adv_pc(1)
            .tick(8)
    }
    fn ld_h_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_ld_r_d8(cpu, REG_H, mem.read(cpu.HL()))
            .adv_pc(1)
            .tick(8)
    }
    fn ld_l_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_ld_r_d8(cpu, REG_L, mem.read(cpu.HL()))
            .adv_pc(1)
            .tick(8)
    }
    fn ld_a_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_ld_r_d8(cpu, REG_A, mem.read(cpu.HL()))
            .adv_pc(1)
            .tick(8)
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
        reg[REG_A] = mem.read(cpu.BC());
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
        reg[REG_A] = mem.read(cpu.DE());
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
        reg[REG_A] = mem.read(combine(high, low));

        CPUState { reg, ..cpu }.tick(16).adv_pc(3)
    }

    //   ld   (BC),A      02         8 ----
    // ----------------------------------------------------------------------------
    fn ld_BC_a(cpu: CPUState, mem: &mut Memory) -> CPUState {
        mem.write(cpu.BC(), cpu.reg[REG_A]);
        CPUState {
            pc: cpu.pc + 1,
            tsc: cpu.tsc + 8,
            ..cpu
        }
    }

    //   ld   (DE),A      12         8 ----
    // ----------------------------------------------------------------------------
    fn ld_DE_a(cpu: CPUState, mem: &mut Memory) -> CPUState {
        mem.write(cpu.DE(), cpu.reg[REG_A]);
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
        mem.write(addr, cpu.reg[REG_A]);

        cpu.tick(16).adv_pc(3)
    }

    //   ld   (nn),SP      08 nn nn        20 ----
    // ----------------------------------------------------------------------------
    fn ld_A16_sp(low: Byte, high: Byte, cpu: CPUState, mem: &mut Memory) -> CPUState {
        let addr = combine(high, low);
        mem.write(addr + 1, hi(cpu.sp));
        mem.write(addr + 0, lo(cpu.sp));

        cpu.tick(20).adv_pc(3)
    }

    //   ld   A,(FF00+n)  F0 nn     12 ---- read from io-port n (memory FF00+n)
    // ----------------------------------------------------------------------------
    fn ld_a_FF00_A8(cpu: CPUState, mem: &Memory, off: Byte) -> CPUState {
        let mut reg = cpu.reg;
        reg[REG_A] = mem.read(MEM_IO_PORTS + off as Word);
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
        mem.write(MEM_IO_PORTS + off as Word, cpu.reg[REG_A]);
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
        reg[REG_A] = mem.read(MEM_IO_PORTS + reg[REG_C] as Word);
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
        mem.write(MEM_IO_PORTS + cpu.reg[REG_C] as Word, cpu.reg[REG_A]);
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
        mem.write(cpu.HL(), reg[REG_A]);

        let (hli, _) = cpu.HL().overflowing_add(1);
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
        reg[REG_A] = mem.read(cpu.HL());

        let (hli, _) = cpu.HL().overflowing_add(1);
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
        mem.write(cpu.HL(), reg[REG_A]);

        let (hld, _) = cpu.HL().overflowing_sub(1);
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
        reg[REG_A] = mem.read(cpu.HL());

        let (hld, _) = cpu.HL().overflowing_sub(1);
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

    fn impl_push_rr(cpu: CPUState, mem: &mut Memory, reg_hi: usize, reg_lo: usize) -> CPUState {
        let val = combine(cpu.reg[reg_hi], cpu.reg[reg_lo]);
        let cpu_pushed = push_d16(cpu, mem, val);
        CPUState {
            pc: cpu.pc + 1,
            tsc: cpu.tsc + 16,
            ..cpu_pushed
        }
    }

    fn impl_pop_rr(cpu: CPUState, mem: &Memory, reg_hi: usize, reg_lo: usize) -> CPUState {
        let (cpu_popped, pval) = pop_d16(cpu, mem);

        let mut reg = cpu_popped.reg;
        reg[reg_hi] = hi(pval);
        reg[reg_lo] = lo(pval);
        if reg_lo == FLAGS {
            // special case: FLAGS low nibble is always 0
            reg[reg_lo] &= 0xF0;
        }

        CPUState {
            pc: cpu.pc + 1,
            tsc: cpu.tsc + 12,
            reg,
            ..cpu_popped
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
    // ----------------------------------------------------------------------------
    fn ld_sp_hl(cpu: CPUState) -> CPUState {
        CPUState {
            sp: cpu.HL(),
            ..cpu
        }
        .tick(8)
        .adv_pc(1)
    }

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

    /// makes some half-carry operations easier if we think in 4bit terms
    const fn alu_add_4bit(a: Byte, b: Byte, c_in: bool) -> (Byte, bool) {
        let ret: Byte = (a & 0x0F) + (b & 0x0F) + (c_in as u8);
        let c_out = ret & 0x10 != 0;
        (ret & 0x0F, c_out)
    }

    const fn impl_add_sub_c(cpu: CPUState, arg: Byte, fl_n: Byte, c_read: bool) -> CPUState {
        // for SUB, we invert the arg (1s complement) and add. Note that this will result in
        // an answer that is off-by-one. To correct the result, we leverage the internal
        // carry-out flag in the ALU. In other words, SUB just becomes an ADD with all args
        // inverted, including carries.
        let arg = if fl_n != 0 { !arg } else { arg };

        // inverting the main carry-in:
        let c_in: bool = c_read && (cpu.reg[FLAGS] & FL_C != 0);
        let c_in = if fl_n != 0 { !c_in } else { c_in };

        let (lo, c_out_lo) = alu_add_4bit(cpu.reg[REG_A], arg, c_in);
        // c_out_lo would be doubly-inverted while doing the operation
        // so we don't invert here. However, we do still need to keep
        // track of the half carry flag, so keep in mind that it DOES
        // get inverted before flags are set
        let (hi, c_out_hi) = alu_add_4bit(cpu.reg[REG_A] >> 4, arg >> 4, c_out_lo);

        // inverting the result of the carry-outs if this was a SUB operation (FL_N != 0):
        let c_out_hi = if fl_n != 0 { !c_out_hi } else { c_out_hi };
        let c_out_lo = if fl_n != 0 { !c_out_lo } else { c_out_lo };

        let mut reg = cpu.reg;
        reg[REG_A] = hi << 4 | lo;
        reg[FLAGS] = fl_z(reg[REG_A]) | fl_n | fl_set(FL_H, c_out_lo) | fl_set(FL_C, c_out_hi);

        CPUState { reg, ..cpu }
    }

    const fn impl_add_sub(cpu: CPUState, arg: Byte, fl_n: Byte) -> CPUState {
        // add/sub where we don't care about the carry
        impl_add_sub_c(cpu, arg, fl_n, false)
    }

    const fn impl_adc_sbc(cpu: CPUState, arg: Byte, fl_n: Byte) -> CPUState {
        // add/sub where we do care about the carry
        impl_add_sub_c(cpu, arg, fl_n, true)
    }

    #[test]
    fn test_adc() {
        let INITIAL: CPUState = CPUState::new();
        let cpu = CPUState {
            reg: [0xCD, 0x11, 0, 0, 0, 0, 0x00, 0x01],
            ..INITIAL
        };
        let cpu_c = CPUState {
            reg: [0xCD, 0x11, 0, 0, 0, 0, FL_C, 0x01],
            ..INITIAL
        };

        assert_eq!(
            impl_adc_sbc(cpu, 0xFE, 0).reg[REG_A],
            0xFF,
            "failed plain 0xFE"
        );
        assert_eq!(impl_adc_sbc(cpu_c, 0xFE, 0).reg[REG_A], 0x00);
        assert_eq!(
            impl_adc_sbc(cpu_c, 0xFE, 0).reg[FLAGS],
            FL_Z | FL_H | FL_C,
            "failed carry 0xFE"
        );

        assert_eq!(impl_adc_sbc(cpu, 0x0F, 0).reg[REG_A], 0x10);
        assert_eq!(
            impl_adc_sbc(cpu, 0x0F, 0).reg[FLAGS],
            FL_H,
            "failed plain 0x0F"
        );

        assert_eq!(impl_adc_sbc(cpu_c, 0x0F, 0).reg[REG_A], 0x11);
        assert_eq!(
            impl_adc_sbc(cpu_c, 0x0F, 0).reg[FLAGS],
            FL_H,
            "failed carry 0x0F"
        );

        assert_eq!(
            impl_adc_sbc(cpu, 0x01, 0).reg[REG_A],
            0x02,
            "failed plain 0x01"
        );
        assert_eq!(
            impl_adc_sbc(cpu, 0x01, 0).reg[FLAGS],
            0,
            "failed plain 0x01"
        );

        assert_eq!(
            impl_adc_sbc(cpu_c, 0x01, 0).reg[REG_A],
            0x03,
            "failed carry 0x01"
        );
        assert_eq!(
            impl_adc_sbc(cpu_c, 0x01, 0).reg[FLAGS],
            0,
            "failed carry flags 0x01"
        );
    }

    const fn impl_and(cpu: CPUState, arg: Byte) -> CPUState {
        // z010
        let mut reg = cpu.reg;

        reg[REG_A] &= arg;
        reg[FLAGS] = fl_z(reg[REG_A]) | FL_H;

        CPUState { reg, ..cpu }
    }
    const fn impl_xor(cpu: CPUState, arg: Byte) -> CPUState {
        // z000
        let mut reg = cpu.reg;

        reg[REG_A] ^= arg;
        reg[FLAGS] = fl_z(reg[REG_A]);

        CPUState { reg, ..cpu }
    }
    const fn impl_or(cpu: CPUState, arg: Byte) -> CPUState {
        // z000
        let mut reg = cpu.reg;

        reg[REG_A] |= arg;
        reg[FLAGS] = fl_z(reg[REG_A]);

        CPUState { reg, ..cpu }
    }
    const fn impl_inc_dec(cpu: CPUState, dst: usize, flag_n: Byte) -> CPUState {
        // z0h- for inc
        // z1h- for dec
        let mut reg = cpu.reg;
        let (h, (res, _c)) = if flag_n != 0 {
            (reg[dst] & 0x0F == 0x00, reg[dst].overflowing_sub(1))
        } else {
            (reg[dst] & 0x0F == 0x0F, reg[dst].overflowing_add(1))
        };

        let flags = reg[FLAGS] & FL_C // maintain the carry, we'll set the rest
    | fl_z(res)
    | flag_n
    | fl_set(FL_H, h);

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
        let flagged = impl_add_sub(cpu, arg, FL_N);
        reg[FLAGS] = flagged.reg[FLAGS];
        CPUState { reg, ..flagged }
    }

    //   add  A,r         8x         4 z0hc A=A+r
    // ----------------------------------------------------------------------------
    const fn add_r(cpu: CPUState, src: usize) -> CPUState {
        impl_add_sub(cpu, cpu.reg[src], 0).adv_pc(1).tick(4)
    }

    //   add  A,n         C6 nn      8 z0hc A=A+n
    // ----------------------------------------------------------------------------
    const fn add_d8(cpu: CPUState, d8: Byte) -> CPUState {
        impl_add_sub(cpu, d8, 0).adv_pc(2).tick(8)
    }

    //   add  A,(HL)      86         8 z0hc A=A+(HL)
    // ----------------------------------------------------------------------------
    fn add_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_add_sub(cpu, mem.read(cpu.HL()), 0).adv_pc(1).tick(8)
    }

    //   adc  A,r         8x         4 z0hc A=A+r+cy
    // ----------------------------------------------------------------------------
    const fn adc_r(cpu: CPUState, src: usize) -> CPUState {
        impl_adc_sbc(cpu, cpu.reg[src], 0).adv_pc(1).tick(4)
    }

    //   adc  A,n         CE nn      8 z0hc A=A+n+cy
    // ----------------------------------------------------------------------------
    const fn adc_d8(cpu: CPUState, d8: Byte) -> CPUState {
        impl_adc_sbc(cpu, d8, 0).adv_pc(2).tick(8)
    }

    //   adc  A,(HL)      8E         8 z0hc A=A+(HL)+cy
    // ----------------------------------------------------------------------------
    fn adc_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_adc_sbc(cpu, mem.read(cpu.HL()), 0).adv_pc(1).tick(8)
    }

    //   sub  r           9x         4 z1hc A=A-r
    // ----------------------------------------------------------------------------
    const fn sub_r(cpu: CPUState, src: usize) -> CPUState {
        impl_add_sub(cpu, cpu.reg[src], FL_N).adv_pc(1).tick(4)
    }

    //   sub  n           D6 nn      8 z1hc A=A-n
    // ----------------------------------------------------------------------------
    const fn sub_d8(cpu: CPUState, d8: Byte) -> CPUState {
        impl_add_sub(cpu, d8, FL_N).adv_pc(2).tick(8)
    }

    //   sub  (HL)        96         8 z1hc A=A-(HL)
    // ----------------------------------------------------------------------------
    fn sub_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_add_sub(cpu, mem.read(cpu.HL()), FL_N)
            .adv_pc(1)
            .tick(8)
    }

    //   sbc  A,r         9x         4 z1hc A=A-r-cy
    // ----------------------------------------------------------------------------
    const fn sbc_r(cpu: CPUState, src: usize) -> CPUState {
        impl_adc_sbc(cpu, cpu.reg[src], FL_N).adv_pc(1).tick(4)
    }
    //   sbc  A,n         DE nn      8 z1hc A=A-n-cy
    // ----------------------------------------------------------------------------
    const fn sbc_d8(cpu: CPUState, d8: Byte) -> CPUState {
        impl_adc_sbc(cpu, d8, FL_N).adv_pc(2).tick(8)
    }
    //   sbc  A,(HL)      9E         8 z1hc A=A-(HL)-cy
    // ----------------------------------------------------------------------------
    fn sbc_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_adc_sbc(cpu, mem.read(cpu.HL()), FL_N)
            .adv_pc(1)
            .tick(8)
    }

    //   and  r           Ax         4 z010 A=A & r
    // ----------------------------------------------------------------------------
    const fn and_r(cpu: CPUState, src: usize) -> CPUState {
        impl_and(cpu, cpu.reg[src]).adv_pc(1).tick(4)
    }

    //   and  n           E6 nn      8 z010 A=A & n
    // ----------------------------------------------------------------------------
    const fn and_d8(cpu: CPUState, d8: Byte) -> CPUState {
        impl_and(cpu, d8).adv_pc(2).tick(8)
    }

    //   and  (HL)        A6         8 z010 A=A & (HL)
    // ----------------------------------------------------------------------------
    fn and_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_and(cpu, mem.read(cpu.HL())).adv_pc(1).tick(8)
    }

    //   xor  r           Ax         4 z000
    // ----------------------------------------------------------------------------
    const fn xor_r(cpu: CPUState, src: usize) -> CPUState {
        impl_xor(cpu, cpu.reg[src]).adv_pc(1).tick(4)
    }

    //   xor  n           EE nn      8 z000
    // ----------------------------------------------------------------------------
    const fn xor_d8(cpu: CPUState, d8: Byte) -> CPUState {
        impl_xor(cpu, d8).adv_pc(2).tick(8)
    }

    //   xor  (HL)        AE         8 z000
    // ----------------------------------------------------------------------------
    fn xor_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_xor(cpu, mem.read(cpu.HL())).adv_pc(1).tick(8)
    }

    //   or   r           Bx         4 z000 A=A | r
    // ----------------------------------------------------------------------------
    const fn or_r(cpu: CPUState, src: usize) -> CPUState {
        impl_or(cpu, cpu.reg[src]).adv_pc(1).tick(4)
    }

    //   or   n           F6 nn      8 z000 A=A | n
    // ----------------------------------------------------------------------------
    const fn or_d8(cpu: CPUState, d8: Byte) -> CPUState {
        impl_or(cpu, d8).adv_pc(2).tick(8)
    }

    //   or   (HL)        B6         8 z000 A=A | (HL)
    // ----------------------------------------------------------------------------
    fn or_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_or(cpu, mem.read(cpu.HL())).adv_pc(1).tick(8)
    }

    //   cp   r           Bx         4 z1hc compare A-r
    // ----------------------------------------------------------------------------
    const fn cp_r(cpu: CPUState, src: usize) -> CPUState {
        impl_cp(cpu, cpu.reg[src]).adv_pc(1).tick(4)
    }

    //   cp   n           FE nn      8 z1hc compare A-n
    // ----------------------------------------------------------------------------
    const fn cp_d8(cpu: CPUState, d8: Byte) -> CPUState {
        impl_cp(cpu, d8).adv_pc(2).tick(8)
    }

    //   cp   (HL)        BE         8 z1hc compare A-(HL)
    // ----------------------------------------------------------------------------
    fn cp_HL(cpu: CPUState, mem: &Memory) -> CPUState {
        impl_cp(cpu, mem.read(cpu.HL())).adv_pc(1).tick(8)
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
            mem.read(cpu.HL()) & 0x0F == 0x0F,
            mem.read(cpu.HL()).overflowing_add(1),
        );

        let flags = reg[FLAGS] & FL_C // maintain the carry, we'll set the rest
    | fl_z(res)
    | fl_set(FL_H, h);
        reg[FLAGS] = flags;

        mem.write(cpu.HL(), res);

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
            mem.read(cpu.HL()) & 0x0F == 0x00,
            mem.read(cpu.HL()).overflowing_sub(1),
        );

        let flags = reg[FLAGS] & FL_C // maintain the carry, we'll set the rest
            | fl_z(res)
            | FL_N
            | fl_set(FL_H, h);
        reg[FLAGS] = flags;

        mem.write(cpu.HL(), res);

        CPUState { reg, ..cpu }.adv_pc(1).tick(12)
    }

    //   daa              27         4 z-0x decimal adjust akku
    // ----------------------------------------------------------------------------
    const fn daa(cpu: CPUState) -> CPUState {
        let mut reg = cpu.reg;
        let acc = cpu.reg[REG_A];

        reg[FLAGS] = cpu.reg[FLAGS] & FL_N; // preserve FL_N

        // (previous instruction was a subtraction)
        let prev_sub = cpu.reg[FLAGS] & FL_N != 0;

        // https://ehaskins.com/2018-01-30%20Z80%20DAA/
        let mut offset: Byte = 0x00;
        if cpu.reg[FLAGS] & FL_H != 0 || ((acc & 0x0f) > 0x09 && !prev_sub) {
            offset |= 0x06;
        }
        if cpu.reg[FLAGS] & FL_C != 0 || (acc > 0x99 && !prev_sub) {
            offset |= 0x60;
            reg[FLAGS] |= FL_C;
        }

        reg[REG_A] = if prev_sub {
            let (result, _c) = acc.overflowing_sub(offset);
            result
        } else {
            let (result, _c) = acc.overflowing_add(offset);
            result
        };
        reg[FLAGS] |= fl_z(reg[REG_A]);

        CPUState { reg, ..cpu }.adv_pc(1).tick(4)
    }

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

        let (result, c) = cpu.HL().overflowing_add(rr);
        let half_carries = result ^ (cpu.HL() ^ rr);

        // https://stackoverflow.com/questions/57958631/game-boy-half-carry-flag-and-16-bit-instructions-especially-opcode-0xe8
        // we only test the high byte because of the order of operations of adding (low byte, then high byte).
        // half-carry MAY be set on the low byte, but it doesn't matter for the final result of the flag
        reg[FLAGS] =
            (reg[FLAGS] & FL_Z) | fl_set(FL_H, half_carries & 0x1000 != 0) | fl_set(FL_C, c);
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
    const fn add_sp_r8(cpu: CPUState, arg: SByte) -> CPUState {
        // https://stackoverflow.com/questions/62006764/how-is-xor-applied-when-determining-carry
        let argx = arg as Word;
        let sp0 = cpu.sp;
        let sp1 = cpu.sp.wrapping_add(argx); // == sp0 ^ argx ^ (c << 1)
        let carry = sp1 ^ (sp0 ^ argx); // removes sp0 and argx from sp1, leaving c << 1

        let mut reg = cpu.reg;
        reg[FLAGS] = 0 | 0 | fl_set(FL_H, carry & 0x0010 != 0) | fl_set(FL_C, carry & 0x0100 != 0);

        CPUState {
            sp: sp1,
            reg,
            ..cpu
        }
        .tick(16)
        .adv_pc(2)
    }

    //   ld   HL,SP+dd  F8          12 00hc HL = SP +/- dd ;dd is 8bit signed number
    const fn ld_hl_sp_r8(cpu: CPUState, arg: SByte) -> CPUState {
        // https://stackoverflow.com/questions/62006764/how-is-xor-applied-when-determining-carry
        let argx = arg as Word;
        let hl = cpu.sp.wrapping_add(argx); // == sp ^ argx ^ (c << 1)
        let carry = hl ^ (cpu.sp ^ argx); // removes sp and argx from hl, leaving c << 1

        let mut reg = cpu.reg;
        reg[FLAGS] = 0 | 0 | fl_set(FL_H, carry & 0x0010 != 0) | fl_set(FL_C, carry & 0x0100 != 0);
        reg[REG_H] = hi(hl);
        reg[REG_L] = lo(hl);

        CPUState { reg, ..cpu }.tick(12).adv_pc(2)
    }

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
    // ----------------------------------------------------------------------------
    const fn rrca(cpu: CPUState) -> CPUState {
        let mut reg = cpu.reg;
        reg[FLAGS] = (cpu.reg[REG_A] & 1) << 4;
        reg[REG_A] = cpu.reg[REG_A].rotate_right(1);
        CPUState {
            pc: cpu.pc + 1,
            tsc: cpu.tsc + 4,
            reg,
            ..cpu
        }
    }

    //   rra            1F           4 000c rotate akku right through carry
    // ----------------------------------------------------------------------------
    const fn rra(cpu: CPUState) -> CPUState {
        let mut reg = cpu.reg;
        reg[FLAGS] = (cpu.reg[REG_A] & 1) << 4;
        reg[REG_A] = (cpu.reg[REG_A].rotate_right(1) & 0x7F) | ((cpu.reg[FLAGS] & FL_C) << 3);
        CPUState {
            pc: cpu.pc + 1,
            tsc: cpu.tsc + 4,
            reg,
            ..cpu
        }
    }

    //   rlc  r         CB 0x        8 z00c rotate left
    // ----------------------------------------------------------------------------
    const fn rlc_r(cpu: CPUState, dst: usize) -> CPUState {
        let mut reg = cpu.reg;

        let result = reg[dst].rotate_left(1);

        reg[dst] = result;
        reg[FLAGS] = fl_z(result) | fl_set(FL_C, (result & 1) != 0);

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   rlc  (HL)      CB 06       16 z00c rotate left
    // ----------------------------------------------------------------------------
    fn rlc_hl(cpu: CPUState, mem: &mut Memory) -> CPUState {
        let mut reg = cpu.reg;
        let addr = cpu.HL();
        let cur = mem.read(addr);

        let result = cur.rotate_left(1);

        mem.write(addr, result);
        reg[FLAGS] = fl_z(result) | fl_set(FL_C, (result & 1) != 0);

        CPUState { reg, ..cpu }.adv_pc(2).tick(16)
    }

    //   rl   r         CB 1x        8 z00c rotate left through carry
    // ----------------------------------------------------------------------------
    const fn rl_r(cpu: CPUState, dst: usize) -> CPUState {
        let mut reg = cpu.reg;

        reg[dst] = (cpu.reg[dst].rotate_left(1) & 0xFE) | ((cpu.reg[FLAGS] & FL_C) >> 4);
        reg[FLAGS] = (cpu.reg[dst] & 0x80) >> 3 | fl_z(reg[dst]);

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   rl   (HL)      CB 16       16 z00c rotate left through carry
    // ----------------------------------------------------------------------------
    fn rl_hl(cpu: CPUState, mem: &mut Memory) -> CPUState {
        let mut reg = cpu.reg;
        let addr = cpu.HL();
        let cur = mem.read(addr);

        mem.write(
            addr,
            (cur.rotate_left(1) & 0xFE) | ((cpu.reg[FLAGS] & FL_C) >> 4),
        );
        reg[FLAGS] = (cur & 0x80) >> 3 | fl_z(mem.read(addr));

        CPUState { reg, ..cpu }.adv_pc(2).tick(16)
    }

    //   rrc  r         CB 0x        8 z00c rotate right
    // ----------------------------------------------------------------------------
    const fn rrc_r(cpu: CPUState, dst: usize) -> CPUState {
        let mut reg = cpu.reg;

        let result = reg[dst].rotate_right(1);
        let fl_c = fl_set(FL_C, (cpu.reg[dst] & 1) != 0);

        reg[dst] = result;
        reg[FLAGS] = fl_z(result) | fl_c;

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   rrc  (HL)      CB 0E       16 z00c rotate right
    // ----------------------------------------------------------------------------
    fn rrc_hl(cpu: CPUState, mem: &mut Memory) -> CPUState {
        let mut reg = cpu.reg;
        let addr = cpu.HL();
        let cur = mem.read(addr);

        let result = cur.rotate_right(1);

        mem.write(addr, result);
        reg[FLAGS] = fl_z(result) | fl_set(FL_C, (cur & 1) != 0);

        CPUState { reg, ..cpu }.adv_pc(2).tick(16)
    }

    //   rr   r         CB 1x        8 z00c rotate right through carry
    // ----------------------------------------------------------------------------
    const fn rr_r(cpu: CPUState, dst: usize) -> CPUState {
        let mut reg = cpu.reg;

        let fl_c: Byte = fl_set(FL_C, cpu.reg[dst] & 1 != 0);

        reg[dst] = (cpu.reg[dst].rotate_right(1) & 0x7F) | ((cpu.reg[FLAGS] & FL_C) << 3);
        reg[FLAGS] = fl_c | fl_z(reg[dst]);

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   rr   (HL)      CB 1E       16 z00c rotate right through carry
    // ----------------------------------------------------------------------------
    fn rr_hl(cpu: CPUState, mem: &mut Memory) -> CPUState {
        let mut reg = cpu.reg;
        let addr = cpu.HL();
        let cur = mem.read(addr);

        let result = (cur.rotate_right(1) & 0x7F) | ((cpu.reg[FLAGS] & FL_C) << 3);

        mem.write(addr, result);
        reg[FLAGS] = fl_z(result) | fl_set(FL_C, cur & 1 != 0);

        CPUState { reg, ..cpu }.adv_pc(2).tick(16)
    }

    //   sla  r         CB 2x        8 z00c shift left arithmetic (b0=0)
    // ----------------------------------------------------------------------------
    const fn sla_r(cpu: CPUState, dst: usize) -> CPUState {
        let mut reg = cpu.reg;

        reg[dst] = reg[dst] << 1;
        reg[FLAGS] = fl_z(reg[dst]) | fl_set(FL_C, cpu.reg[dst] & 0x80 != 0);

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   sla  (HL)      CB 26       16 z00c shift left arithmetic (b0=0)
    // ----------------------------------------------------------------------------
    fn sla_hl(cpu: CPUState, mem: &mut Memory) -> CPUState {
        let mut reg = cpu.reg;
        let addr = cpu.HL();
        let cur = mem.read(addr);

        let result = cur << 1;

        mem.write(addr, result);
        reg[FLAGS] = fl_z(result) | fl_set(FL_C, cur & 0x80 != 0);

        CPUState { reg, ..cpu }.adv_pc(2).tick(16)
    }

    //   swap r         CB 3x        8 z000 exchange low/hi-nibble
    // ----------------------------------------------------------------------------
    const fn swap_r(cpu: CPUState, dst: usize) -> CPUState {
        let mut reg = cpu.reg;

        reg[dst] = (reg[dst] >> 4) | (reg[dst] << 4);
        reg[FLAGS] = fl_z(reg[dst]);

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   swap (HL)      CB 36       16 z000 exchange low/hi-nibble
    // ----------------------------------------------------------------------------
    fn swap_hl(cpu: CPUState, mem: &mut Memory) -> CPUState {
        let mut reg = cpu.reg;
        let addr = cpu.HL();
        let cur = mem.read(addr);

        let result = (cur >> 4) | (cur << 4);

        mem.write(addr, result);
        reg[FLAGS] = fl_z(result);

        CPUState { reg, ..cpu }.adv_pc(2).tick(16)
    }

    //   sra  r         CB 2x        8 z00c shift right arithmetic (b7=b7)
    // ----------------------------------------------------------------------------
    const fn sra_r(cpu: CPUState, dst: usize) -> CPUState {
        let mut reg = cpu.reg;

        reg[dst] = (cpu.reg[dst] & 0x80) | reg[dst] >> 1;
        reg[FLAGS] = fl_z(reg[dst]) | fl_set(FL_C, cpu.reg[dst] & 1 != 0);

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   sra  (HL)      CB 2E       16 z00c shift right arithmetic (b7=b7)
    // ----------------------------------------------------------------------------
    fn sra_hl(cpu: CPUState, mem: &mut Memory) -> CPUState {
        let mut reg = cpu.reg;
        let addr = cpu.HL();
        let cur = mem.read(addr);

        let result = (cur & 0x80) | cur >> 1;

        mem.write(addr, result);
        reg[FLAGS] = fl_z(result) | fl_set(FL_C, cur & 1 != 0);

        CPUState { reg, ..cpu }.adv_pc(2).tick(16)
    }

    //   srl  r         CB 3x        8 z00c shift right logical (b7=0)
    // ----------------------------------------------------------------------------
    const fn srl_r(cpu: CPUState, dst: usize) -> CPUState {
        let mut reg = cpu.reg;

        reg[dst] = reg[dst] >> 1;
        reg[FLAGS] = fl_z(reg[dst]) | fl_set(FL_C, cpu.reg[dst] & 1 != 0);

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   srl  (HL)      CB 3E       16 z00c shift right logical (b7=0)
    // ----------------------------------------------------------------------------
    fn srl_hl(cpu: CPUState, mem: &mut Memory) -> CPUState {
        let mut reg = cpu.reg;
        let addr = cpu.HL();
        let cur = mem.read(addr);

        let result = cur >> 1;

        mem.write(addr, result);
        reg[FLAGS] = fl_z(result) | fl_set(FL_C, cur & 1 != 0);

        CPUState { reg, ..cpu }.adv_pc(2).tick(16)
    }

    // GMB Singlebit Operation Commands
    // ============================================================================
    //   bit  n,r       CB xx        8 z01- test bit n
    // ----------------------------------------------------------------------------
    const fn bit_r(cpu: CPUState, bit: Byte, dst: usize) -> CPUState {
        let mut reg = cpu.reg;

        let mask = 1 << bit;
        reg[FLAGS] = fl_z(cpu.reg[dst] & mask) | FL_H | cpu.reg[FLAGS] & FL_C;

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   bit  n,(HL)    CB xx       12 z01- test bit n
    // ----------------------------------------------------------------------------
    fn bit_hl(cpu: CPUState, mem: &mut Memory, bit: Byte) -> CPUState {
        let mut reg = cpu.reg;
        let addr = cpu.HL();
        let cur = mem.read(addr);

        let mask = 1 << bit;
        reg[FLAGS] = fl_z(cur & mask) | FL_H | (cpu.reg[FLAGS] & FL_C);

        CPUState { reg, ..cpu }.adv_pc(2).tick(12)
    }

    //   set  n,r       CB xx        8 ---- set bit n
    // ----------------------------------------------------------------------------
    const fn set_r(cpu: CPUState, bit: Byte, dst: usize) -> CPUState {
        let mut reg = cpu.reg;

        let mask = 1 << bit;
        reg[dst] |= mask;

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   set  n,(HL)    CB xx       16 ---- set bit n
    // ----------------------------------------------------------------------------
    fn set_hl(cpu: CPUState, mem: &mut Memory, bit: Byte) -> CPUState {
        let reg = cpu.reg;
        let addr = cpu.HL();

        let mask = 1 << bit;
        mem.write(addr, mem.read(addr) | mask);

        CPUState { reg, ..cpu }.adv_pc(2).tick(16)
    }

    //   res  n,r       CB xx        8 ---- reset bit n
    // ----------------------------------------------------------------------------
    const fn res_n_r(cpu: CPUState, n: Byte, r: usize) -> CPUState {
        let mut reg = cpu.reg;

        let mask = 1 << n;
        reg[r] &= !mask;

        CPUState { reg, ..cpu }.adv_pc(2).tick(8)
    }

    //   res  n,(HL)    CB xx       16 ---- reset bit n
    // ----------------------------------------------------------------------------
    fn res_n_hl(cpu: CPUState, mem: &mut Memory, n: Byte) -> CPUState {
        let reg = cpu.reg;
        let addr = cpu.HL();

        let mask = 1 << n;
        mem.write(addr, mem.read(addr) & !mask);

        CPUState { reg, ..cpu }.adv_pc(2).tick(16)
    }

    #[test]
    fn test_res_n_r() {
        let cpu = CPUState {
            reg: [0xFE, 0, 0, 0, 0, 0, 0, 0],
            ..CPUState::new()
        };
        assert_eq!(res_n_r(cpu, 0, REG_B).reg[REG_B], 0b11111110);
        assert_eq!(res_n_r(cpu, 1, REG_B).reg[REG_B], 0b11111100);
        assert_eq!(res_n_r(cpu, 2, REG_B).reg[REG_B], 0b11111010);
        assert_eq!(res_n_r(cpu, 3, REG_B).reg[REG_B], 0b11110110);
        assert_eq!(res_n_r(cpu, 4, REG_B).reg[REG_B], 0b11101110);
        assert_eq!(res_n_r(cpu, 5, REG_B).reg[REG_B], 0b11011110);
        assert_eq!(res_n_r(cpu, 6, REG_B).reg[REG_B], 0b10111110);
        assert_eq!(res_n_r(cpu, 7, REG_B).reg[REG_B], 0b01111110);
    }

    // GMB CPU-Controlcommands
    // ============================================================================
    //   ccf            3F           4 -00c cy=cy xor 1
    const fn ccf(cpu: CPUState) -> CPUState {
        let mut reg = cpu.reg;
        reg[FLAGS] = reg[FLAGS] & FL_Z | 0 | 0 | (reg[FLAGS] ^ FL_C) & FL_C;

        CPUState { reg, ..cpu }.adv_pc(1).tick(4)
    }

    //   scf            37           4 -001 cy=1
    const fn scf(cpu: CPUState) -> CPUState {
        let mut reg = cpu.reg;
        reg[FLAGS] = reg[FLAGS] & FL_Z | 0 | 0 | FL_C;

        CPUState { reg, ..cpu }.adv_pc(1).tick(4)
    }

    #[test]
    fn test_ccf_scf() {
        let cpu = CPUState::new();
        let cpu_zeroed = CPUState {
            reg: [0, 0, 0, 0, 0, 0, 0, 0],
            ..cpu
        };

        let ccf0 = ccf(cpu);
        let ccf1 = ccf(ccf0);
        assert_eq!(ccf0.reg[FLAGS] & FL_C, (!(cpu.reg[FLAGS] & FL_C)) & FL_C);
        assert_eq!(ccf1.reg[FLAGS] & FL_C, cpu.reg[FLAGS] & FL_C);

        assert_eq!(scf(cpu).reg[FLAGS] & FL_C, FL_C);
        assert_eq!(scf(cpu_zeroed).reg[FLAGS] & FL_C, FL_C);
    }

    //   nop            00           4 ---- no operation
    // ----------------------------------------------------------------------------
    const fn nop(cpu: CPUState) -> CPUState {
        cpu.adv_pc(1).tick(4)
    }

    //   halt           76         N*4 ---- halt until interrupt occurs (low power)
    const fn halt(cpu: CPUState) -> CPUState {
        CPUState { halt: true, ..cpu }.adv_pc(1).tick(4)
    }

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
        let cpu_pushed = push_d16(cpu, mem, cpu.pc);
        CPUState {
            pc: combine(high, low),
            ..cpu_pushed
        }
    }

    //   call f,nn      xx nn nn 24;12 ---- conditional call if nz,z,nc,c
    // ----------------------------------------------------------------------------
    fn call_f_d16(low: Byte, high: Byte, cpu: CPUState, mem: &mut Memory, op: Byte) -> CPUState {
        // 0xC4: NZ | 0xD4: NC | 0xCC: Z | 0xDC: C
        let do_call = match op {
            0xC4 => (cpu.reg[FLAGS] & FL_Z) == 0,
            0xD4 => (cpu.reg[FLAGS] & FL_C) == 0,
            0xCC => (cpu.reg[FLAGS] & FL_Z) != 0,
            0xDC => (cpu.reg[FLAGS] & FL_C) != 0,
            _ => panic!("call_f_d16 unreachable"),
        };
        if do_call {
            call_d16(low, high, cpu, mem)
        } else {
            cpu.adv_pc(3).tick(12)
        }
    }

    //   ret            C9          16 ---- return, PC=(SP), SP=SP+2
    // ----------------------------------------------------------------------------
    fn ret(cpu: CPUState, mem: &Memory) -> CPUState {
        let (cpu_popped, pval) = pop_d16(cpu, mem);
        CPUState {
            pc: pval,
            tsc: cpu.tsc + 16,
            ..cpu_popped
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

        let cpu_pushed = push_d16(cpu, mem, cpu.pc);

        CPUState {
            pc: rst_addr as Word,
            ..cpu_pushed
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
        let flags = mem.read(IF);
        mem.write(IF, flags & !fl_int); // acknowledge the request flag (set to 0)
                                        // push current position to stack to prepare for jump

        let cpu_pushed = push_d16(cpu, mem, cpu.pc);

        CPUState {
            ime: mem.read(IF) != 0, // only lock the ime if we're handling the final request
            // todo: acc: this behavior is incorrect, the ime should remain locked while handling the
            // SET OF interrupt requests that were enabled at the time of the handler invocation
            // e.g. if FL_INT_VSYNC and FL_INT_JOYPAD are requested then the interrupt handler
            // should execute both (in order of priority) but NOT execute any newly requested
            // interrupts until those are handled.
            pc: vec_int,
            ..cpu_pushed
        }
        .tick(20) // https://gbdev.io/pandocs/Interrupts.html#interrupt-handling
    }

    // ============================================================================
    // memory functions
    // ============================================================================
    pub fn request_interrupt(mem: &mut Memory, int_flag: Byte) {
        mem.write(IF, mem.read(IF) | int_flag);
    }

    fn mem_inc(mem: &mut Memory, loc: Word) -> (Byte, bool) {
        let (result, overflow) = mem.read(loc).overflowing_add(1);
        mem.write(loc, result);
        (result, overflow)
    }

    fn tima_reset(mem: &mut Memory) {
        mem.write(TIMA, mem.read(TMA));
    }

    fn tac_enabled(mem: &Memory) -> bool {
        mem.read(TAC) & 0b100 != 0
    }

    fn tac_cycles_per_inc(mem: &Memory) -> Result<u64, &'static str> {
        match mem.read(TAC) & 0b11 {
            0b00 => Ok(1024),
            0b01 => Ok(16),
            0b10 => Ok(64),
            0b11 => Ok(256),
            _ => Err("Invalid TAC clock setting"),
        }
    }

    #[cfg(test)]
    mod tests_cpu {
        use super::*;
        use crate::dbg::*;

        // tsc: 0,
        // //    B     C     D     E     H     L     fl    A
        // reg: [0x00, 0x13, 0x00, 0xD8, 0x01, 0x4D, 0xB0, 0x01],
        // sp: 0xFFFE,
        // pc: 0x0000,
        // ime: false,
        const INITIAL: CPUState = CPUState::new();

        macro_rules! assert_eq_flags {
            ($left:expr, $right:expr) => {
                assert_eq!(
                    $left,
                    $right,
                    "flags: expected {}, actual {}",
                    str_flags($right),
                    str_flags($left)
                )
            };
        }

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
        fn test_xor_r() {
            let result = xor_r(INITIAL, REG_A);
            assert_eq!(result.reg[REG_A], 0x00);
            assert_eq!(result.reg[FLAGS], 0x80);
        }

        #[test]
        fn test_xor_bc() {
            let state = CPUState {
                reg: [0xCD, 0x11, 0, 0, 0, 0, 0x80, 0x01],
                ..INITIAL
            };
            assert_eq!(xor_r(state, REG_B).reg[REG_A], 0xCC);
            assert_eq!(xor_r(state, REG_C).reg[REG_A], 0x10);
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
            for op in 0x40..0x80 {
                if op & 0x0F == 0x06 || op & 0x0F == 0x0E || op & 0xF0 == 0x70 {
                    continue;
                }
                let dst_idx = ((op - 0x40) / 0x08) as usize;
                let src_idx = (op % 0x08) as usize;
                assert_eq!(ld_r_r(INITIAL, op).reg[dst_idx], INITIAL.reg[src_idx]);
            }
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
            assert_eq!(
                impl_add_sub(INITIAL, 0xFF, 0).reg[REG_A],
                0x00,
                "failed 0xff"
            );
            assert_eq!(
                impl_add_sub(INITIAL, 0xFF, 0).reg[FLAGS],
                FL_Z | FL_H | FL_C,
                "failed 0xff flags"
            );

            assert_eq!(
                impl_add_sub(INITIAL, 0x0F, 0).reg[REG_A],
                0x10,
                "failed 0x0f"
            );
            assert_eq!(
                impl_add_sub(INITIAL, 0x0F, 0).reg[FLAGS],
                FL_H,
                "failed 0x0f flags"
            );

            assert_eq!(
                impl_add_sub(INITIAL, 0x01, 0).reg[REG_A],
                0x02,
                "failed 0x01"
            );
            assert_eq!(
                impl_add_sub(INITIAL, 0x01, 0).reg[FLAGS],
                0x00,
                "failed 0x01 flags"
            );
        }

        #[test]
        fn test_add_hl_rr() {
            assert_eq!(
                add_hl_bc(INITIAL).HL(),
                INITIAL.HL().overflowing_add(INITIAL.BC()).0
            );
            assert_eq!(
                add_hl_de(INITIAL).HL(),
                INITIAL.HL().overflowing_add(INITIAL.DE()).0
            );
            assert_eq!(
                add_hl_hl(INITIAL).HL(),
                INITIAL.HL().overflowing_add(INITIAL.HL()).0
            );
            assert_eq!(
                add_hl_sp(INITIAL).HL(),
                INITIAL.HL().overflowing_add(INITIAL.sp).0
            );

            // test flags (-0hc)
            // todo: fix, this test itself was incorrect (was checking the wrong flags)
            // let mut reg = INITIAL.reg;
            // reg[REG_H] = 0x00;
            // reg[REG_L] = 0xFF;
            // reg[REG_B] = 0x00;
            // reg[REG_C] = 0x01;
            // assert_eq!(
            //     add_hl_bc(CPUState { reg, ..INITIAL }).reg[FLAGS],
            //     INITIAL.reg[FLAGS] & FL_Z | 0 | FL_H | 0
            // );
            // reg[REG_H] = 0xFF;
            // assert_eq!(
            //     add_hl_bc(CPUState { reg, ..INITIAL }).reg[FLAGS],
            //     INITIAL.reg[FLAGS] & FL_Z | 0 | FL_H | FL_C
            // );
        }

        #[test]
        fn test_add_HL() {
            let mut mem = Memory::new();
            let cpu = CPUState {
                reg: [0, 0, 0, 0, 0, 0x01, 0, 0x01],
                ..INITIAL
            };
            mem.write(cpu.HL(), 0x0F);
            assert_eq!(add_HL(cpu, &mem).reg[REG_A], 0x10);
            assert_eq!(add_HL(cpu, &mem).reg[FLAGS], FL_H);
        }

        #[test]
        fn test_inc_HL() {
            let mut mem = Memory::new();
            let mut cpu = CPUState {
                reg: [0, 0, 0, 0, 0, 0x01, FL_Z | FL_N | FL_H | FL_C, 0x01],
                ..INITIAL
            };

            let initial: Byte = 0x0E;
            mem.write(cpu.HL(), initial);
            cpu = inc_HL(cpu, &mut mem);

            assert_eq!(mem.read(cpu.HL()), initial + 1);
            assert_eq!(cpu.reg[FLAGS], FL_C); // FL_C remains untouched by this operation

            // increment again, this time 0x0F should half-carry into 0x10
            cpu = inc_HL(cpu, &mut mem);
            assert_eq!(mem.read(cpu.HL()), initial + 2);
            assert_eq!(cpu.reg[FLAGS], FL_H | FL_C); // FL_H from half-carry

            // reset value to 0xFF, confirm we get a FL_Z flag on overflow
            mem.write(cpu.HL(), 0xFF);
            cpu = inc_HL(cpu, &mut mem);
            assert_eq!(mem.read(cpu.HL()), 0);
            assert_eq!(cpu.reg[FLAGS], FL_Z | FL_H | FL_C); // todo: should FL_H get set here? it does! but should it?
        }

        #[test]
        fn test_call_d16() {
            let mut mem = Memory::new();
            let result = call_d16(0x01, 0x02, INITIAL, &mut mem);
            assert_eq!(
                mem.read(INITIAL.sp - 0),
                hi(INITIAL.adv_pc(3).pc),
                "failed high check"
            );
            assert_eq!(
                mem.read(INITIAL.sp - 1),
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
            let mut mem = Memory::new();
            mem.write(cpu.HL(), cpu.reg[REG_L]);

            assert_eq_flags!(cp_r(cpu, REG_B).reg[FLAGS], FL_N);
            assert_eq_flags!(cp_r(cpu, REG_C).reg[FLAGS], FL_N);
            assert_eq_flags!(cp_r(cpu, REG_D).reg[FLAGS], FL_N | FL_H);
            assert_eq_flags!(cp_r(cpu, REG_E).reg[FLAGS], FL_N | FL_H);
            assert_eq_flags!(cp_r(cpu, REG_H).reg[FLAGS], FL_Z | FL_N);
            assert_eq_flags!(cp_r(cpu, REG_L).reg[FLAGS], FL_N | FL_H | FL_C);
            assert_eq_flags!(cp_r(cpu, REG_A).reg[FLAGS], FL_Z | FL_N);

            assert_eq_flags!(cp_d8(cpu, 0x12).reg[FLAGS], FL_N | FL_H | FL_C);
            assert_eq_flags!(cp_HL(cpu, &mem).reg[FLAGS], FL_N | FL_H | FL_C);
        }

        #[test]
        fn test_sub() {
            let cpu = CPUState {
                //    B     C     D     E     H     L     fl    A
                reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0x12, FL_C, 0x11],
                ..INITIAL
            };
            assert_eq!(sub_r(cpu, REG_B).reg[REG_A], 0x11);
            assert_eq!(sub_r(cpu, REG_C).reg[REG_A], 0x10);
            assert_eq!(sub_r(cpu, REG_D).reg[REG_A], 0x0F);
            let result = sub_r(cpu, REG_D).reg[FLAGS];
            assert_eq!(
                result,
                FL_N | FL_H,
                "expected {}, got {}",
                str_flags(FL_N | FL_H),
                str_flags(result)
            );
            assert_eq!(sub_r(cpu, REG_E).reg[REG_A], 0x0E);
            assert_eq!(sub_r(cpu, REG_H).reg[REG_A], 0x00);
            assert_eq!(sub_r(cpu, REG_H).reg[FLAGS], FL_Z | FL_N);
            assert_eq!(sub_r(cpu, REG_L).reg[REG_A], 0xFF);
            assert_eq!(sub_r(cpu, REG_L).reg[FLAGS], FL_N | FL_H | FL_C);
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
            assert_eq!(inc_sp(cpu).sp, ROM_ENTRY);
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
            let mut mem = Memory::new();
            impl_ld_HL_d8(cpu, &mut mem, 0x22);
            assert_eq!(mem.read(cpu.HL()), 0x22);
        }

        #[test]
        fn test_ldi() {
            let cpu = CPUState {
                //    B     C     D     E     H     L     fl    A
                reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0x22, FL_C, 0xAA],
                ..INITIAL
            };
            let mut mem = Memory::new();
            mem.write(cpu.HL(), 0x0F);
            assert_eq!(ldi_HL_a(cpu, &mut mem).reg[REG_H], cpu.reg[REG_H]);
            assert_eq!(ldi_HL_a(cpu, &mut mem).reg[REG_L], cpu.reg[REG_L] + 1);
            assert_eq!(mem.read(cpu.HL()), cpu.reg[REG_A]);
        }

        #[test]
        fn test_ldd() {
            let cpu = CPUState {
                //    B     C     D     E     H     L     fl    A
                reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0x22, FL_C, 0xAA],
                ..INITIAL
            };
            let mut mem = Memory::new();
            mem.write(cpu.HL(), 0x0F);
            assert_eq!(ldd_HL_a(cpu, &mut mem).reg[REG_H], cpu.reg[REG_H]);
            assert_eq!(ldd_HL_a(cpu, &mut mem).reg[REG_L], cpu.reg[REG_L] - 1);
            assert_eq!(mem.read(cpu.HL()), cpu.reg[REG_A]);
        }

        #[test]
        fn test_push() {
            let cpu = CPUState {
                //    B     C     D     E     H     L     fl    A
                reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0xFF, FL_C, 0xAA],
                ..INITIAL
            };
            let mut mem = Memory::new();
            assert_eq!(push_bc(cpu, &mut mem).sp, cpu.sp - 2);
            assert_eq!(mem.read(cpu.sp - 2), cpu.reg[REG_B]);
            assert_eq!(mem.read(cpu.sp - 1), cpu.reg[REG_C]);
        }

        #[test]
        fn test_pop() {
            let cpu = CPUState {
                sp: 0xDEAD,
                ..INITIAL
            };

            let mut mem = Memory::new();
            mem.write(0xDEAD + 1, 0xAD);
            mem.write(0xDEAD + 2, 0xDE);

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
            let mut mem = Memory::new();
            mem.write(0xFFFE, 0xBE);
            mem.write(0xFFFD, 0xEF);
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
            let mut mem = Memory::new();
            mem.write(0xBBCC, 0xAB);
            mem.write(0xDDEE, 0xAD);
            assert_eq!(ld_a_BC(cpu, &mem).reg[REG_A], mem.read(0xBBCC));
            assert_eq!(ld_a_DE(cpu, &mem).reg[REG_A], mem.read(0xDDEE));

            ld_BC_a(cpu, &mut mem);
            assert_eq!(mem.read(0xBBCC), 0xAA);

            ld_DE_a(cpu, &mut mem);
            assert_eq!(mem.read(0xDDEE), 0xAA);

            ld_A16_a(0xCE, 0xFA, cpu, &mut mem);
            assert_eq!(mem.read(0xFACE), 0xAA);
        }

        #[test]
        fn test_FF00_offsets() {
            let cpu = CPUState {
                //    B     C     D     E     H     L     fl    A
                reg: [0x00, 0xCC, 0x02, 0x03, 0x11, 0xFF, FL_C, 0xAA],
                ..INITIAL
            };
            let mut mem = Memory::new();
            mem.write(0xFF00, 0);
            mem.write(0xFF01, 1);
            mem.write(0xFF02, 2);
            mem.write(0xFF03, 3);
            mem.write(0xFFCC, 0xCC);
            assert_eq!(ld_a_FF00_A8(cpu, &mem, 0x02).reg[REG_A], 0x02);
            assert_eq!(ld_a_FF00_C(cpu, &mem).reg[REG_A], 0xCC);
            ld_FF00_A8_a(0x01, cpu, &mut mem);
            assert_eq!(mem.read(0xFF01), cpu.reg[REG_A]);

            ld_FF00_C_a(cpu, &mut mem);
            assert_eq!(mem.read(0xFFCC), cpu.reg[REG_A]);
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

            assert_eq!(rl_r(cpu, REG_B).reg[REG_B], 0x80);
            assert_eq!(rl_r(cpu, REG_C).reg[REG_C], 0x80);
            assert_eq!(rl_r(cpu, REG_D).reg[REG_D], 0x80);
            assert_eq!(rl_r(cpu, REG_E).reg[REG_E], 0x80);
            assert_eq!(rl_r(cpu, REG_H).reg[REG_H], 0x80);
            assert_eq!(rl_r(cpu, REG_L).reg[REG_L], 0x80);
            assert_eq!(rl_r(cpu, REG_A).reg[REG_A], 0x00);
            assert_eq!(rl_r(cpu, REG_A).reg[FLAGS], FL_Z | FL_C);
            assert_eq!(rl_r(rl_r(cpu, REG_A), REG_A).reg[REG_A], 0x01);
        }

        #[test]
        fn test_bit() {
            let cpu = CPUState {
                //    B          C       D       E       H       L      fl     A
                reg: [1 << 0, 1 << 1, 1 << 2, 1 << 3, 1 << 4, 1 << 5, FL_C, 1 << 7],
                ..INITIAL
            };
            assert_eq!(bit_r(cpu, 7, REG_H).reg[FLAGS], FL_H | cpu.reg[FLAGS]);
            assert_eq!(set_r(cpu, 7, REG_H).reg[REG_H], cpu.reg[REG_H] | 0x80);
        }

        #[test]
        fn test_timers() {
            let mut mem = Memory::new();
            mem.write(TIMA, 0);
            mem.write(TMA, 0);
            mem.write(TAC, 0);
            assert_eq!(tac_enabled(&mem), false);
            mem.write(TAC, 0b100); // (enabled, 1024 cycles per tick)
            assert_eq!(tac_enabled(&mem), true);

            let new_timers = update_clocks(HardwareTimers::new(), &mut mem, 1024);
            assert_eq!(new_timers.timer, 0);
            assert_eq!(mem.read(TIMA), 1);

            tima_reset(&mut mem);
            assert_eq!(mem.read(TIMA), 0);

            mem.write(TAC, 0b111); // (enabled, 256 cycles per tick)
            let new_timers = update_clocks(HardwareTimers::new(), &mut mem, 1024);
            assert_eq!(new_timers.timer, 0);
            assert_eq!(mem.read(TIMA), 4);

            mem.write(TMA, 0xFF);
            tima_reset(&mut mem);
            assert_eq!(mem.read(TIMA), mem.read(TMA));

            mem.write(TMA, 0xAA);
            assert_ne!(mem.read(IF), FL_INT_TIMER);
            let _even_newer_timers = update_clocks(new_timers, &mut mem, 256);
            // should have overflowed as we just set it to 0xFF moments ago
            assert_eq!(mem.read(TIMA), 0xAA);
            assert_eq!(mem.read(IF), FL_INT_TIMER);

            // TODO test DIV
            // TODO can we test frame timer? it's set up differently...
        }

        #[test]
        fn test_lcd() {
            let mut mem = Memory::new();
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

            let rot_b = rlc_r(cpu, REG_B);
            assert_eq!(rot_b.reg[REG_B], 0x00);
            assert_eq!(rot_b.reg[FLAGS], FL_Z);

            let rot_c = rlc_r(cpu, REG_C);
            assert_eq!(rot_c.reg[REG_C], 0x02);
            assert_eq!(rot_c.reg[FLAGS], 0x00);

            let rot_d = rlc_r(cpu, REG_D);
            assert_eq!(rot_d.reg[REG_D], 0x01);
            assert_eq!(rot_d.reg[FLAGS], FL_C);

            let rot_l = rlc_r(cpu, REG_L);
            assert_eq!(rot_l.reg[REG_L], 0xFF);
            assert_eq!(rot_l.reg[FLAGS], FL_C);
        }
    }
}

pub mod memory {
    use crate::bits::{combine, hi, lo};
    use crate::cpu::CPUState;
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
    pub const SB: Word = 0xFF01;
    pub const SC: Word = 0xFF02;
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
        // --- debug ---
        pub doctor: bool,
    }
    impl Memory {
        pub fn new() -> Memory {
            let mut mem = Memory {
                data: [0; MEM_SIZE],
                dma_req: false,
                doctor: false,
            };
            mem.write(TIMA, 0x00);
            mem.write(TMA, 0x00);
            mem.write(TAC, 0x00);
            mem.write(NR10, 0x80);
            mem.write(NR11, 0xBF);
            mem.write(NR12, 0xF3);
            mem.write(NR14, 0xBF);
            mem.write(NR21, 0x3F);
            mem.write(NR22, 0x00);
            mem.write(NR24, 0xBF);
            mem.write(NR30, 0x7F);
            mem.write(NR31, 0xFF);
            mem.write(NR32, 0x9F);
            mem.write(NR33, 0xBF);
            mem.write(NR41, 0xFF);
            mem.write(NR42, 0x00);
            mem.write(NR43, 0x00);
            mem.write(NR44, 0xBF);
            mem.write(NR50, 0x77);
            mem.write(NR51, 0xF3);
            mem.write(NR52, 0xF1);
            mem.write(LCDC, 0x91);
            mem.write(SCY, 0x00);
            mem.write(SCX, 0x00);
            mem.write(LYC, 0x00);
            mem.write(BGP, 0xFC);
            mem.write(OBP0, 0xFF);
            mem.write(OBP1, 0xFF);
            mem.write(WY, 0x00);
            mem.write(WX, 0x00);
            mem.write(IE, 0x00);
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
        pub fn write(&mut self, addr: Word, val: Byte) {
            let blocked = vec![
                DIV,
                // 0xFF41, // stat
            ];
            if !blocked.contains(&addr) {
                // println!("[${:04X}]={:02X}", addr, val);
            }
            match addr {
                JOYP => {
                    self[addr] |= 0x30 & val; // lower nibble is read only
                }
                _ => self[addr] = val,
            }
        }
        pub fn read(&self, addr: Word) -> Byte {
            match addr {
                JOYP => {
                    let bitset = if 0x30 & self[addr] == 0x30 { 0x0F } else { 0 };
                    self[addr] | bitset
                }
                IE => self[addr] & 0x1F,
                IF => self[addr] & 0x1F,
                _ => self[addr],
            }
        }
    }
    impl Index<Word> for Memory {
        type Output = Byte;
        fn index(&self, index: Word) -> &Self::Output {
            match index {
                LY => {
                    if self.doctor {
                        &0x90
                    } else {
                        &self.data[index as usize]
                    }
                } // for debugger https://robertheaton.com/gameboy-doctor/
                _ => &self.data[index as usize],
            }
        }
    }
    impl IndexMut<Word> for Memory {
        fn index_mut(&mut self, index: Word) -> &mut Self::Output {
            match index {
                DMA => {
                    // println!("[DMA] 0x{:X}", self[index]);
                    self.dma_req = true;
                }
                SB => {
                    // Serial port bytes
                    // println!("[SB] {}", self[index] as char);
                }
                // LCDC => println!("[LCDC]"),
                _ => {
                    // println!("[{:04X}] {:02X}", index, self[index]);
                }
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

    // --- pushes and popses ---
    pub fn push_d8(cpu: CPUState, mem: &mut Memory, val: Byte) -> CPUState {
        let sp = cpu.sp - 1;
        mem.write(sp, val);
        CPUState { sp, ..cpu }
    }
    pub fn push_d16(cpu: CPUState, mem: &mut Memory, val: Word) -> CPUState {
        let sp = cpu.sp - 2;
        mem.write(sp + 1, hi(val));
        mem.write(sp + 0, lo(val));
        CPUState { sp, ..cpu }
    }
    pub fn pop_d8(cpu: CPUState, mem: &Memory) -> (CPUState, Byte) {
        let val: Byte = mem.read(cpu.sp);
        let sp = cpu.sp + 1;
        (CPUState { sp, ..cpu }, val)
    }
    pub fn pop_d16(cpu: CPUState, mem: &Memory) -> (CPUState, Word) {
        let h: Byte = mem.read(cpu.sp + 1);
        let l: Byte = mem.read(cpu.sp + 0);
        let val: Word = combine(h, l);
        let sp = cpu.sp + 2;
        (CPUState { sp, ..cpu }, val)
    }
}

pub mod types {
    pub type Byte = u8;
    pub type Word = u16;
    pub type SByte = i8;
    pub type SWord = i16;

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

#[rustfmt::skip]
pub mod lcd {
    use crate::bits::*;
    use crate::cpu::*;
    use crate::dbg::dump;
    use crate::memory::*;
    use crate::types::*;
    use minifb::Window;

    // lcdc
    pub const LCDC_BIT_ENABLE                     :Byte = BIT_7;
    pub const LCDC_BIT_WINDOW_TILE_MAP_SELECT     :Byte = BIT_6;
    pub const LCDC_BIT_WINDOW_ENABLE              :Byte = BIT_5;
    pub const LCDC_BIT_BG_WINDOW_TILE_DATA_SELECT :Byte = BIT_4;
    pub const LCDC_BIT_BG_TILE_MAP_SELECT         :Byte = BIT_3;
    pub const LCDC_BIT_OBJ_SIZE                   :Byte = BIT_2;
    pub const LCDC_BIT_OBJ_ENABLE                 :Byte = BIT_1;
    pub const LCDC_BIT_BG_WINDOW_ENABLE           :Byte = BIT_0;

    // lcd status
    pub const STAT_BIT_NULL              :Byte = BIT_7;
    pub const STAT_BIT_LYC_INT_SELECT    :Byte = BIT_6;
    pub const STAT_BIT_MODE_2_INT_SELECT :Byte = BIT_5;
    pub const STAT_BIT_MODE_1_INT_SELECT :Byte = BIT_4;
    pub const STAT_BIT_MODE_0_INT_SELECT :Byte = BIT_3;
    pub const STAT_BIT_LY_LYC_EQ         :Byte = BIT_2;
    pub const STAT_MASK_PPU_MODE         :Byte = 0b011;

    // object attribute flags
    pub const OAM_BIT_PRIORITY           :Byte = BIT_7;
    pub const OAM_BIT_FLIP_Y             :Byte = BIT_6;
    pub const OAM_BIT_FLIP_X             :Byte = BIT_5;
    pub const OAM_BIT_DMG_PAL            :Byte = BIT_4; // dmg only
    pub const OAM_BIT_BANK               :Byte = BIT_3;
    pub const OAM_MASK_CGB_PAL           :Byte = 0b111; // color gameboy only
    pub const OBJ_ATTR_SIZE              :Word = 4;

    // other constants
    pub const PPU_TILE_WIDTH             :usize = 8;
    
    pub struct Sprite {
        idx: Word
    }
    impl Sprite {
        fn y(&self, mem: &Memory) -> Byte {
            mem[MEM_OAM + self.idx * OBJ_ATTR_SIZE + 0]
        }
        fn x(&self, mem: &Memory) -> Byte {
            mem[MEM_OAM + self.idx * OBJ_ATTR_SIZE + 1]
        }
        fn tile(&self, mem: &Memory) -> Byte {
            if mem[LCDC] & LCDC_BIT_OBJ_SIZE != 0 {
                // todo: CGB can reference VRAM in bank 0 or bank 1
                mem[MEM_OAM + self.idx * OBJ_ATTR_SIZE + 2] & 0xFE // masked, ignore least sig. bit (hardware-enforced)
            } else {
                mem[MEM_OAM + self.idx * OBJ_ATTR_SIZE + 2]
            }
        }
        fn flags(&self, mem: &Memory) -> Byte {
            mem[MEM_OAM + self.idx * OBJ_ATTR_SIZE + 3]
        }
        fn hit(&self, mem: &Memory) -> Byte {
            if self.x(mem) != 0 {
                let scanline = mem[LY] + 16;
                let height = if mem[LCDC] & LCDC_BIT_OBJ_SIZE != 0 { 16 } else { 8 };
                if scanline >= self.y(mem) && scanline < self.y(mem) + height {
                    // todo: does this work for double height?
                    let yy = self.y(mem);
                    if self.flags(mem) & OAM_BIT_FLIP_Y != 0 {
                        (height - 1) - (scanline - yy)
                    } 
                    else 
                    {
                        scanline - yy
                    }
                } else {
                    SPRITE_NOT_HIT
                }
            } else {
                SPRITE_NOT_HIT
            }
        }
    }

    const SPRITE_NOT_HIT: Byte = 0xFF;
    pub struct SpriteHit {
        sprite: Sprite,
        line: Byte
    }

    pub struct Display {
        buffer: Vec<u32>,
        buffer_sprites: Vec<SpriteHit>,
        lcd_timing: u64,
        // debug
        pub doctor: bool,
        doctor_LY: Byte,
    }

    impl Display {
        pub fn new() -> Display {
            Display {
                buffer: vec![0; GB_SCREEN_WIDTH * GB_SCREEN_HEIGHT],
                buffer_sprites: vec![],
                lcd_timing: 0,
                doctor: false,
                doctor_LY: 0
            }
        }

        pub fn update(&mut self, mem: &mut Memory, window: &mut Window, dt: u64 ) {
            self.lcd_timing += dt;
            lcd_compare_ly_lyc(mem);
            match lcd_mode(&mem) {
                // oam search
                2 => {
                    if self.lcd_timing >= TICKS_PER_OAM_SEARCH {
                        self.buffer_sprites.clear();
                        for n in 0..40 {
                            let s = Sprite { idx: n };
                            let l = s.hit(&mem);
                            if self.buffer_sprites.len() < 10 && l != SPRITE_NOT_HIT {
                                self.buffer_sprites.push(SpriteHit{sprite: s, line: l});
                            }
                        }
                        set_lcd_mode(3, mem);
                        self.lcd_timing -= TICKS_PER_OAM_SEARCH;
                    }
                }
                // vram io
                3 => {
                    if self.lcd_timing >= TICKS_PER_VRAM_IO {
                        // draw the scanline
                        // ===========================================
                        let cur_line: Byte = if self.doctor { self.doctor_LY } else { mem[LY] };
                        let ln_start: usize = GB_SCREEN_WIDTH * cur_line as usize;
                        let ln_end: usize = ln_start + GB_SCREEN_WIDTH;

                        // draw background
                        // -------------------------------------------
                        // todo: acc: this code is inaccurate, LCDC can actually be modified mid-scanline
                        // but cerboy currently only draws the line in a single shot (instead of per-dot)
                        let bg_tilemap_start: Word = if bit_test(3, mem[LCDC]) {
                            0x9C00
                        } else {
                            0x9800
                        };
                        let (bg_signed_addressing, bg_tile_data_start) = if bit_test(4, mem[LCDC]) {
                            (false, MEM_VRAM as Word)
                        } else {
                            // in signed addressing the 0 tile is at 0x9000
                            (true, MEM_VRAM + 0x1000 as Word)
                            // (true, MEM_VRAM + 0x0800 as Word) // <--- actual range starts at 0x8800 but that is -127, not zero
                        };
                        let (bg_y, _) = mem[SCY].overflowing_add(cur_line);
                        let bg_tile_line = bg_y as Word % 8;

                        for (c, it) in self.buffer[ln_start..ln_end].iter_mut().enumerate() {
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
                            let bg_tile_line_data = ppu_decode_tile_line(mem[bg_tile_line_offset], mem[bg_tile_line_offset + 1]);
                            let bg_tile_current_pixel = 7 - ((c as Byte + mem[SCX]) % 8);
                            *it = palette_lookup(bg_tile_line_data[bg_tile_current_pixel as usize], mem[BGP], &PAL_CLASSIC);
                        }

                        // draw sprites
                        // FE00-FE9F   Sprite Attribute Table (OAM)
                        // -------------------------------------------
                        for (c, it) in self.buffer[ln_start..ln_end].iter_mut().enumerate() {
                            // the x attr for the sprite is an offset from -8 to allow
                            // for off-screen (left side) positions.
                            // We can simply adjust the value of c on this line 
                            // to account for this.
                            let c_off = (c + 8) as Byte;
                            // nyctrip
                            // todo: non-cgb: lower-x sprites are drawn on top of higher-x
                            for hit in self.buffer_sprites.iter() {
                                let spr = &hit.sprite;
                                if c_off >= spr.x(&mem) && c_off < (spr.x(&mem) + 8) {
                                    let data_size_mul = if hit.line > 7 { 2 } else { 1 }; // for double height sprites
                                    let spr_tile_data_offset = spr.tile(&mem) as Word * BYTES_PER_TILE * data_size_mul;
                                    let tile_hit_line = hit.line % 8;
                                    // from here we can work in a tile-local context
                                    let spr_tile_data_line_offset = 
                                        MEM_VRAM + 
                                        spr_tile_data_offset + 
                                        tile_hit_line as Word * 2;
                                    let spr_tile_line_data = ppu_decode_tile_line(mem[spr_tile_data_line_offset], mem[spr_tile_data_line_offset + 1]);
                                    let spr_pix = 7 - (c_off - spr.x(&mem));
                                    if spr_tile_line_data[spr_pix as usize] != 0 {
                                        // todo: draw in correct priority order for opaque pixels
                                        *it = palette_lookup(spr_tile_line_data[spr_pix as usize], mem[OBP0], &PAL_ICE_CREAM); // todo: OBP1
                                    }
                                }
                            }
                        }

                        // draw window
                        // -------------------------------------------
                        // for i in buffer[ln_start..ln_end].iter_mut() {}

                        // ===========================================

                        set_lcd_mode(0, mem);
                        self.lcd_timing -= TICKS_PER_VRAM_IO;
                    }
                }
                // hblank
                0 => {
                    let cur_line: &mut Byte = if self.doctor { &mut self.doctor_LY } else { &mut mem[LY] };
                    if self.lcd_timing >= TICKS_PER_HBLANK {
                        *cur_line += 1;
                        self.lcd_timing -= TICKS_PER_HBLANK;
                        if *cur_line == GB_SCREEN_HEIGHT as Byte {
                            // values 144 to 153 are vblank
                            request_interrupt(mem, FL_INT_VBLANK);
                            set_lcd_mode(1, mem);
                        } else {
                            set_lcd_mode(2, mem);
                        }
                    }
                }
                // vblank
                1 => {
                    let cur_line: &mut Byte = if self.doctor { &mut self.doctor_LY } else { &mut mem[LY] };
                    *cur_line = (GB_SCREEN_HEIGHT as u64 + self.lcd_timing / TICKS_PER_SCANLINE) as Byte;
                    if self.lcd_timing >= TICKS_PER_VBLANK {
                        *cur_line = 0;
                        set_lcd_mode(2, mem);
                        self.lcd_timing -= TICKS_PER_VBLANK;

                        window
                            .update_with_buffer(&self.buffer, GB_SCREEN_WIDTH, GB_SCREEN_HEIGHT)
                            .unwrap();

                        if self.doctor {
                            dump("mem.bin", &mem).unwrap()
                        }
                    }
                }
                _ => panic!("invalid LCD mode"),
            };
        }
    }
    
    pub fn lcd_compare_ly_lyc(mem: &mut Memory) -> bool {
        // https://gbdev.io/pandocs/STAT.html#ff45--lyc-ly-compare
        let equal = mem.read(LY) == mem.read(LYC);
        let comparison = bit_set(STAT_BIT_LY_LYC_EQ, mem.read(STAT), equal);
        mem.write(STAT, comparison);
        if equal && comparison & STAT_BIT_LYC_INT_SELECT != 0 {
            // if LYC int select is enabled, request an interrupt
            request_interrupt(mem, FL_INT_STAT);
        }
        equal
    }

    pub fn lcd_mode(mem: &Memory) -> Byte {
        mem.read(STAT) & STAT_MASK_PPU_MODE
    }

    pub fn set_lcd_mode(mode: Byte, mem: &mut Memory) {
        mem.write(STAT, (mem.read(STAT) & !STAT_MASK_PPU_MODE) | (mode & STAT_MASK_PPU_MODE));
    }

    pub fn ppu_decode_tile_line(low: Byte, high: Byte) -> [Byte; PPU_TILE_WIDTH] {
        let mut result = [0; PPU_TILE_WIDTH];
        for i in 0..PPU_TILE_WIDTH {
            let mask = 1 << i;
            let masked_low = (low & mask) >> i;
            let masked_high = if i > 0 {
                (high & mask) >> (i - 1)
            } else {
                (high & mask) << 1
            };
            result[i] = masked_high | masked_low;
        }
        result
    }
}

pub mod decode {
    use crate::cpu::*;
    use crate::types::*;

    // https://gb-archive.github.io/salvage/decoding_gbz80_opcodes/Decoding%20Gamboy%20Z80%20Opcodes.html
    // https://www.pastraiser.com/cpu/gameboy/gameboy_opcodes.html

    // used for CB decoding, some bit functions reference (HL) instead of a register
    pub const ADR_HL: usize = 6;
    pub const R_ID: [usize; 8] = [REG_B, REG_C, REG_D, REG_E, REG_H, REG_L, ADR_HL, REG_A];

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

    // bit masks
    pub const BIT_0: Byte = 1 << 0;
    pub const BIT_1: Byte = 1 << 1;
    pub const BIT_2: Byte = 1 << 2;
    pub const BIT_3: Byte = 1 << 3;
    pub const BIT_4: Byte = 1 << 4;
    pub const BIT_5: Byte = 1 << 5;
    pub const BIT_6: Byte = 1 << 6;
    pub const BIT_7: Byte = 1 << 7;

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

    pub const fn fl_set(flag: Byte, set: bool) -> Byte {
        (set as u8) * flag
    }

    pub const fn fl_z(val: Byte) -> Byte {
        fl_set(crate::cpu::FL_Z, val == 0)
    }

    pub const fn bit(idx: Byte, val: Byte) -> Byte {
        (val >> idx) & 1
    }

    pub const fn bit_test(idx: Byte, val: Byte) -> bool {
        bit(idx, val) != 0
    }

    pub const fn bit_set(idx: Byte, val: Byte, set: bool) -> Byte {
        if set {
            val | idx
        } else {
            val & !idx
        }
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
    use std::fs;
    use std::fs::File;
    use std::io::{BufWriter, Write};

    use crate::cpu::*;
    use crate::lcd::*;
    use crate::memory::*;
    use crate::types::*;

    pub struct CPULog {
        cpu: CPUState,
        mem_next: [Byte; 4],
    }

    impl std::fmt::Display for CPULog {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "A:{:02X} F:{:02X} B:{:02X} C:{:02X} D:{:02X} E:{:02X} H:{:02X} L:{:02X} SP:{:04X} PC:{:04X} PCMEM:{:02X},{:02X},{:02X},{:02X}",
                self.cpu.reg[REG_A],
                self.cpu.reg[FLAGS],
                self.cpu.reg[REG_B],
                self.cpu.reg[REG_C],
                self.cpu.reg[REG_D],
                self.cpu.reg[REG_E],
                self.cpu.reg[REG_H],
                self.cpu.reg[REG_L],
                self.cpu.sp,
                self.cpu.pc,
                self.mem_next[0],
                self.mem_next[1],
                self.mem_next[2],
                self.mem_next[3]
            )
        }
    }

    pub fn log_cpu(buffer: &mut Vec<CPULog>, cpu: &CPUState, mem: &Memory) {
        buffer.push(CPULog {
            cpu: cpu.clone(),
            mem_next: [
                mem.read(cpu.pc + 0),
                mem.read(cpu.pc + 1),
                mem.read(cpu.pc + 2),
                mem.read(cpu.pc + 3),
            ],
        });
    }

    pub fn write_cpu_logs(logs: &Vec<CPULog>) -> std::io::Result<()> {
        let f = File::create("cpu.log")?;
        let mut writer = BufWriter::with_capacity(1 << 16, f);
        for log in logs {
            writeln!(writer, "{}", log)?;
        }
        writer.flush()?;
        Ok(())
    }

    pub fn dump(path: &str, mem: &Memory) -> std::io::Result<()> {
        fs::write(path, mem.data)?;
        Ok(())
    }

    const VEC_NAMES: [&str; 5] = ["VBLANK", "STAT", "TIMER", "SERIAL", "JOYPAD"];

    pub const fn str_interrupt(i: Word) -> &'static str {
        let idx = (i - VEC_INT_VBLANK) / 0x08;
        VEC_NAMES[idx as usize]
    }

    pub fn str_flags(flags: Byte) -> String {
        format!(
            "{}{}{}{}",
            if flags & FL_C != 0 { "C" } else { "—" },
            if flags & FL_H != 0 { "H" } else { "—" },
            if flags & FL_N != 0 { "N" } else { "—" },
            if flags & FL_Z != 0 { "Z" } else { "—" },
        )
    }

    #[rustfmt::skip]
    pub fn print_lcdc(mem: &Memory) {
        // print LCDC diagnostics
        let lcdc_v = mem.read(LCDC);
        let lcdc_7 = if lcdc_v & LCDC_BIT_ENABLE != 0                     { " on" }    else { "off" };
        let lcdc_6 = if lcdc_v & LCDC_BIT_WINDOW_TILE_MAP_SELECT != 0     { "0x9C00" } else { "0x9800" };
        let lcdc_5 = if lcdc_v & LCDC_BIT_WINDOW_ENABLE != 0              { " on" }    else { "off" };
        let lcdc_4 = if lcdc_v & LCDC_BIT_BG_WINDOW_TILE_DATA_SELECT != 0 { "0x8000" } else { "0x8800" };
        let lcdc_3 = if lcdc_v & LCDC_BIT_BG_TILE_MAP_SELECT != 0         { "0x9C00" } else { "0x9800" };
        let lcdc_2 = if lcdc_v & LCDC_BIT_OBJ_SIZE != 0                   { "16" }     else { " 8" };
        let lcdc_1 = if lcdc_v & LCDC_BIT_OBJ_ENABLE != 0                 { " on" }    else { "off" };
        let lcdc_0 = if lcdc_v & LCDC_BIT_BG_WINDOW_ENABLE != 0           { " on" }    else { "off" };
        println!("{lcdc_v:#10b} LCDC [scr: {lcdc_7}, wnd_map: {lcdc_6}, wnd: {lcdc_5}, bg/wnd_dat: {lcdc_4}, bg_map: {lcdc_3}, obj_sz: {lcdc_2}, obj: {lcdc_1}, bg: {lcdc_0}]");
    }
}
