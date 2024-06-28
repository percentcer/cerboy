use cerboy::decode::{decode, decodeCB};
use cerboy::memory::*;
use cerboy::types::{Instruction, InstructionCB};

fn main() {
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

    // print rom
    // ------------
    let cart = Cartridge::new(rom_path);
    let mut mem: Memory = Memory::new();
    mem.load_rom(&cart);

    println!(
        "{} | size: {} | banks: {} | ram: {} | hw: {} | dst: {}",
        cart.title(),
        cart.size(),
        cart.num_banks(),
        cart.size_ram(),
        cart.hardware_type(),
        cart.destination_code()
    );

    // hex
    // ------------
    let mut i = 0;
    while i < cart.size() {
        let b = cart[i];
        print!("{b:02X} ");
        i += 1;
        if i % 16 == 0 {
            println!();
        }
    }
    println!();

    // dis
    // ------------
    let mut i = 0;
    while i < cart.size() {
        let inst: Instruction = decode(cart[i]);
        if !inst.valid() {
            i += 1;
            println!("[invalid instruction]");
            continue;
        }
        let argc = inst.len as usize;
        if inst.prefix() {
            i += 1; // (all cb instructions are 1 byte for the prefix and 1 byte for the opcode)
            let cb: InstructionCB = decodeCB(cart[i]);
            let cbinst: Instruction = Instruction::from_cb(&cb);
            println!("{}", cbinst.mnm);
        } else {
            match argc {
                1 => println!("{}", inst.mnm),
                2..=3 => println!("{}", inst.mnm_args(&cart[i + 1..i + argc])),
                _ => panic!("(unreachable) todo: this is getting messy"),
            }
        }
        i += argc;
    }
}
