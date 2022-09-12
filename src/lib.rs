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

    pub struct Instruction {
        pub mnemonic: String,
        pub length: u8, // bytes to read
    }
    impl Instruction {
        // Constructs a new instance of [`Second`].
        // Note this is an associated function - no self.
        pub fn new(text: &str, len: u8) -> Self {
            Self {
                mnemonic: String::from(text),
                length: len,
            }
        }
    }
}

pub mod decode {
    use crate::types::{Byte, Instruction};

    // https://gb-archive.github.io/salvage/decoding_gbz80_opcodes/Decoding%20Gamboy%20Z80%20Opcodes.html

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

    const INV: &'static str = "INVALID";

    // todo: Instruction is constantly allocating heap strings, I feel like there
    // should be a way to do this at compile time but I can't figure it out
    pub fn decode(op: Byte) -> Instruction {
        match x(op) {
            0 => match z(op) {
                0 => match y(op) {
                    0 => Instruction::new("NOP", 1),
                    1 => Instruction::new("LD (nn), SP", 3),
                    2 => Instruction::new("STOP", 1),
                    3 => Instruction::new("JR d", 2),
                    v @ 4..=7 => {
                        let idx: usize = (v - 4) as usize;
                        let flag: &'static str = CC[idx];
                        Instruction {
                            mnemonic: format!("JR {flag}, d"),
                            length: 2,
                        }
                    }
                    _ => Instruction::new(INV, 0),
                },
                1 => {
                    let addr = RP[p(op) as usize];
                    match q(op) {
                        0 => Instruction {
                            mnemonic: format!("LD {addr}, nn"),
                            length: 2,
                        },
                        1 => Instruction {
                            mnemonic: format!("ADD HL, {addr}"),
                            length: 2,
                        },
                        _ => Instruction::new(INV, 0),
                    }
                }
                _ => todo!(),
            },
            _ => todo!()
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
    }
}
