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

    #[derive(PartialEq, Debug)]
    pub struct Instruction {
        pub mnm: String,
        pub len: u8, // bytes to read
    }
    impl Instruction {
        // Constructs a new instance of [`Second`].
        // Note this is an associated function - no self.
        pub fn new(text: &str, len: u8) -> Self {
            Self {
                mnm: String::from(text),
                len,
            }
        }

        pub fn valid(&self) -> bool { 
            self.len > 0
        }

        pub fn prefix(&self) -> bool {
            self.mnm == crate::decode::CBPREFIX
        }
    }
}

pub mod decode {
    use crate::types::{Byte, Instruction};

    // https://gb-archive.github.io/salvage/decoding_gbz80_opcodes/Decoding%20Gamboy%20Z80%20Opcodes.html
    // https://www.pastraiser.com/cpu/gameboy/gameboy_opcodes.html

    // arg tables
    const R: [&'static str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
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
                    3 => Instruction::new("JR d", 2),
                    v @ 4..=7 => {
                        let i: usize = (v - 4) as usize;
                        let _CC_i: &'static str = CC[i];
                        Instruction {
                            mnm: format!("JR {_CC_i}, d"),
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
                        len: 2,
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
                    5 => Instruction::new("ADD SP, d", 2),
                    6 => Instruction::new("LD A, (0xFF00 + n)", 2),
                    7 => Instruction::new("LD HL, SP + d", 2),
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
    pub fn decodeCB(op: Byte) -> Instruction {
        let _ROT_y = ROT[y(op) as usize];
        let _R_z = R[z(op) as usize];
        let _y = y(op);
        match x(op) {
            0 => Instruction {
                mnm: format!("{_ROT_y} {_R_z}"),
                len: 1,
            },
            1 => Instruction {
                mnm: format!("BIT {_y}, {_R_z}"),
                len: 1,
            },
            2 => Instruction {
                mnm: format!("RES {_y}, {_R_z}"),
                len: 1,
            },
            3 => Instruction {
                mnm: format!("SET {_y}, {_R_z}"),
                len: 1,
            },
            _ => Instruction::new(INVALID, 0),
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
        fn test_rot() {
            assert_eq!(decodeCB(0x00), Instruction::new("RLC B", 1));
            assert_eq!(decodeCB(0x10), Instruction::new("RL B", 1));
            assert_eq!(decodeCB(0x20), Instruction::new("SLA B", 1));
            assert_eq!(decodeCB(0x30), Instruction::new("SWAP B", 1));
            assert_eq!(decodeCB(0x40), Instruction::new("BIT 0, B", 1));
            assert_eq!(decodeCB(0x50), Instruction::new("BIT 2, B", 1));
            assert_eq!(decodeCB(0x60), Instruction::new("BIT 4, B", 1));
            assert_eq!(decodeCB(0x70), Instruction::new("BIT 6, B", 1));
            assert_eq!(decodeCB(0x80), Instruction::new("RES 0, B", 1));
            assert_eq!(decodeCB(0x90), Instruction::new("RES 2, B", 1));
            assert_eq!(decodeCB(0xA0), Instruction::new("RES 4, B", 1));
            assert_eq!(decodeCB(0xB0), Instruction::new("RES 6, B", 1));
            assert_eq!(decodeCB(0xC0), Instruction::new("SET 0, B", 1));
            assert_eq!(decodeCB(0xD0), Instruction::new("SET 2, B", 1));
            assert_eq!(decodeCB(0xE0), Instruction::new("SET 4, B", 1));
            assert_eq!(decodeCB(0xF0), Instruction::new("SET 6, B", 1));
        }
    }
}

pub mod io {
    use crate::types::Byte;
    use std::io::Read;

    pub fn init_rom(path: &str) -> Vec<Byte> {
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
    use crate::types::{Byte,Word,SByte};

    pub const HIGH_MASK: Word = 0xFF00;
    pub const LOW_MASK: Word = 0x00FF;

    pub const fn hi(reg: Word) -> Byte {
        (reg >> Byte::BITS) as Byte
    }
    
    pub const fn lo(reg: Word) -> Byte {
        (reg & LOW_MASK) as Byte
    }
    
    pub const fn combine(high: Byte, low: Byte) -> Word {
        (high as Word) << Byte::BITS | (low as Word)
    }

    // can't be const for some reason https://github.com/rust-lang/rust/issues/53605
    pub fn signed(val: Byte) -> SByte {
        unsafe { std::mem::transmute(val) }
    }
}