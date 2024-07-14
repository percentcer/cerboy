#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::identity_op)]
#![feature(const_trait_impl)]

extern crate minifb;
use minifb::{Key, Window, WindowOptions};

extern crate env_logger;

use cerboy::cpu::*;
use cerboy::lcd::*;
use cerboy::memory::*;

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
    let mut lcd: Display = Display::new();
    mem.doctor = args.doctor;
    lcd.doctor = args.doctor;
    mem.load_rom(&cart); // load cartridge

    // todo: boot doesn't work anymore with the new cartridge setup
    // let boot = init_rom("./rom/boot/DMG_ROM.bin");
    // load_rom(&mut mem, &boot);

    let mut timers = HardwareTimers::new();

    // loop
    // ------------
    while window.is_open() && !window.is_key_down(Key::Escape) {
        // update
        // ------------------------------------------------
        if args.doctor {
            println!("A:{:02X} F:{:02X} B:{:02X} C:{:02X} D:{:02X} E:{:02X} H:{:02X} L:{:02X} SP:{:04X} PC:{:04X} PCMEM:{:02X},{:02X},{:02X},{:02X}",
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
                mem[cpu.pc+0],
                mem[cpu.pc+1],
                mem[cpu.pc+2],
                mem[cpu.pc+3]
            )
        }
        let cpu_prev = cpu;
        cpu = match next(cpu_prev, &mut mem) {
            Ok(cpu) => cpu,
            Err(e) => {
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

        // update display
        // ------------------------------------------------
        lcd.update(&mut mem, &mut window, dt_cyc);
    }
}
