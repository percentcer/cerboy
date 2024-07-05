#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::identity_op)]
#![feature(const_trait_impl)]

extern crate minifb;
use minifb::{Key, Window, WindowOptions};

extern crate env_logger;

use cerboy::bits::*;
use cerboy::cpu::*;
use cerboy::dbg::*;
use cerboy::memory::*;
use cerboy::types::*;

use clap::Parser;
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to ROM
    #[arg(short, long)]
    rom: String,

    /// Run in gameboy-doctor mode
    #[arg(short, long, default_value_t = false)]
    doctor: bool,
}

fn main() {
    let args = Args::parse();
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

    // init system
    // ------------
    let cart = Cartridge::new(args.rom.as_str());
    let mut cpu = CPUState::new();
    let mut mem: Memory = Memory::new();
    mem.doctor = args.doctor;
    mem.load_rom(&cart); // load cartridge

    // todo: boot doesn't work anymore with the new cartridge setup
    // let boot = init_rom("./rom/boot/DMG_ROM.bin");
    // load_rom(&mut mem, &boot);

    let mut timers = HardwareTimers::new();
    let mut lcd_timing: u64 = 0;

    // init logging
    // ------------
    let mut cpu_log_lines: Vec<CPULog> = Vec::new();
    let mut DOCTOR_MEM_LY: Byte = 0;

    // loop
    // ------------
    while window.is_open() && !window.is_key_down(Key::Escape) {
        // update
        // ------------------------------------------------
        if args.doctor {
            log_cpu(&mut cpu_log_lines, &cpu, &mem);
        }
        let cpu_prev = cpu;
        cpu = match next(cpu_prev, &mut mem) {
            Ok(cpu) => cpu,
            Err(e) => {
                write_cpu_logs(&cpu_log_lines).expect("Failed to write logs!");
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
                    let cur_line: Byte = if args.doctor { DOCTOR_MEM_LY } else { mem[LY] };
                    // draw the scanline
                    // ===========================================
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
                let cur_line: &mut Byte = if args.doctor { &mut DOCTOR_MEM_LY } else { &mut mem[LY] };
                if lcd_timing >= TICKS_PER_HBLANK {
                    *cur_line += 1;
                    lcd_timing -= TICKS_PER_HBLANK;
                    if *cur_line == GB_SCREEN_HEIGHT as Byte {
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
                let cur_line: &mut Byte = if args.doctor { &mut DOCTOR_MEM_LY } else { &mut mem[LY] };
                *cur_line = (GB_SCREEN_HEIGHT as u64 + lcd_timing / TICKS_PER_SCANLINE) as Byte;
                if lcd_timing >= TICKS_PER_VBLANK {
                    *cur_line = 0;
                    set_lcd_mode(2, &mut mem);
                    lcd_timing -= TICKS_PER_VBLANK;

                    window
                        .update_with_buffer(&buffer, GB_SCREEN_WIDTH, GB_SCREEN_HEIGHT)
                        .unwrap();

                    if args.doctor {
                        dump("mem.bin", &mem).unwrap()
                    }
                }
            }
            _ => panic!("invalid LCD mode"),
        };
    }
    write_cpu_logs(&cpu_log_lines).expect("Failed to write logs!");
}
