#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::identity_op)]
#![feature(const_trait_impl)]

extern crate minifb;
use minifb::{Key, Window, WindowOptions};

extern crate env_logger;

use cerboy::cpu::*;
use cerboy::dbg::*;
use cerboy::memory::*;
use cerboy::types::*;

fn main() {
    env_logger::init();

    // window management
    // -----------------
    let buffer: Vec<u32> = vec![0; GB_SCREEN_WIDTH * GB_SCREEN_HEIGHT];
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
    let cart = Cartridge::new(rom_path);
    let mut cpu = CPUState::new();
    let mut mem: Memory = Memory::new();
    mem.load_rom(&cart); // load cartridge
    
    // todo: boot doesn't work anymore with the new cartridge setup
    // let boot = init_rom("./rom/boot/DMG_ROM.bin");
    // load_rom(&mut mem, &boot);
    
    let mut timers = HardwareTimers::new();
    let mut lcd_timing: u64 = 0;
    
    // init logging
    // ------------
    let mut cpu_log_lines = Vec::new();

    // loop
    // ------------
    while window.is_open() && !window.is_key_down(Key::Escape) {
        // update
        // ------------------------------------------------
        log_cpu(&mut cpu_log_lines, &cpu, &mem).unwrap();
        let cpu_prev = cpu;
        cpu = match next(cpu_prev, &mut mem) {
            Ok(cpu) => cpu,
            Err(e) => {
                cpu_log_lines.push(e.to_string());
                std::fs::write("cpu.log", &mut cpu_log_lines.join("\n")).expect("");
                panic!("{}", e.to_string());
            }
        };
        let dt_cyc = cpu.tsc - cpu_prev.tsc;

        // update memory (e.g. handle any pending DMA transfers)
        // ------------------------------------------------
        mem.update();

        // update timers
        // ------------------------------------------------
        timers = update_clocks(timers, &mut mem, dt_cyc);
        lcd_timing += dt_cyc;

        // render
        // ------------------------------------------------
        match lcd_mode(&mem) {
            // oam search
            2 => {
                if lcd_timing >= TICKS_PER_OAM_SEARCH {
                    // todo: oam search
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
                        *i = palette_lookup(bg_tile_pixel_color_id, mem[BGP], &PAL_CLASSIC);
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

                    // cerboy::dbg::print_lcdc(&mem);
                    dump("mem.bin", &mem).unwrap();
                }
            }
            _ => panic!("invalid LCD mode"),
        };
    }
}

#[cfg(test)]
mod tests_cpu {
    use super::*;

    // tsc: 0,
    // //    B     C     D     E     H     L     fl    A
    // reg: [0x00, 0x13, 0x00, 0xD8, 0x01, 0x4D, 0xB0, 0x01],
    // sp: 0xFFFE,
    // pc: 0x0000,
    // ime: false,
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
        let mut reg = INITIAL.reg;
        reg[REG_H] = 0x00;
        reg[REG_L] = 0xFF;
        reg[REG_B] = 0x00;
        reg[REG_C] = 0x01;
        assert_eq!(
            add_hl_bc(CPUState { reg, ..INITIAL }).reg[FLAGS],
            INITIAL.reg[FLAGS] & FL_Z | 0 | FL_H | 0
        );
        reg[REG_H] = 0xFF;
        assert_eq!(
            add_hl_bc(CPUState { reg, ..INITIAL }).reg[FLAGS],
            INITIAL.reg[FLAGS] & FL_Z | 0 | FL_H | FL_C
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
        let mut mem = Memory::new();
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
        let mut mem = Memory::new();
        let mut cpu = CPUState {
            reg: [0, 0, 0, 0, 0, 0x01, FL_Z | FL_N | FL_H | FL_C, 0x01],
            ..INITIAL
        };

        let initial: Byte = 0x0E;
        mem[cpu.HL()] = initial;
        cpu = inc_HL(cpu, &mut mem);

        assert_eq!(mem[cpu.HL()], initial + 1);
        assert_eq!(cpu.reg[FLAGS], FL_C); // FL_C remains untouched by this operation

        // increment again, this time 0x0F should half-carry into 0x10
        cpu = inc_HL(cpu, &mut mem);
        assert_eq!(mem[cpu.HL()], initial + 2);
        assert_eq!(cpu.reg[FLAGS], FL_H | FL_C); // FL_H from half-carry

        // reset value to 0xFF, confirm we get a FL_Z flag on overflow
        mem[cpu.HL()] = 0xFF;
        cpu = inc_HL(cpu, &mut mem);
        assert_eq!(mem[cpu.HL()], 0);
        assert_eq!(cpu.reg[FLAGS], FL_Z | FL_H | FL_C); // todo: should FL_H get set here? it does! but should it?
    }

    #[test]
    fn test_call_d16() {
        let mut mem = Memory::new();
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
        let mut mem = Memory::new();
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
        assert_eq!(mem[cpu.HL()], 0x22);
    }

    #[test]
    fn test_ldi() {
        let cpu = CPUState {
            //    B     C     D     E     H     L     fl    A
            reg: [0x00, 0x01, 0x02, 0x03, 0x11, 0x22, FL_C, 0xAA],
            ..INITIAL
        };
        let mut mem = Memory::new();
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
        let mut mem = Memory::new();
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
        let mut mem = Memory::new();
        assert_eq!(push_bc(cpu, &mut mem).sp, cpu.sp - 2);
        assert_eq!(mem[cpu.sp - 2], cpu.reg[REG_B]);
        assert_eq!(mem[cpu.sp - 1], cpu.reg[REG_C]);
    }

    #[test]
    fn test_pop() {
        let cpu = CPUState {
            sp: 0xDEAD,
            ..INITIAL
        };

        let mut mem = Memory::new();
        mem[0xDEAD + 1] = 0xAD;
        mem[0xDEAD + 2] = 0xDE;

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
        let mut mem = Memory::new();
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
        let mut mem = Memory::new();
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
        let mut mem = Memory::new();
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
