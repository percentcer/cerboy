use cerboy::io::init_rom;
use cerboy::types::{Byte, Instruction};
use cerboy::decode::{decode, decodeCB};

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
    let rom: Vec<Byte> = init_rom(rom_path);

    // hex
    // ------------
    let mut i = 0;
    while i < rom.len() {
        let b = rom[i];
        print!("{b:02X} ");
        i += 1;
        if i % 16 == 0 {
            print!("\n");
        }
    }
    print!("\n");

    // dis
    // ------------
    let mut i = 0;
    while i < rom.len() {
        let inst: Instruction = decode(rom[i]);
        if inst.valid() {
            if inst.prefix() {
                i += 1;
                let cb: Instruction = decodeCB(rom[i]);
                println!("{}", cb.mnm);
            } else {
                println!("{}", inst.mnm);
            }
            i += inst.len as usize;
        } else {
            println!("[invalid instruction]");
            i += 1
        }
    }
}
