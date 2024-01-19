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
                len: 1
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
                2 => self.mnm.replace("nn", &format!("${:04x}", crate::bits::combine(rom[1], rom[0]))),
                _ => panic!("mnemonic only intended for instructions with args")
            }
        }
    }
}

pub mod decode {
    use crate::types::{Byte, Instruction, InstructionCB};
    use crate::types::{REG_B, REG_C, REG_D, REG_E, REG_H, REG_L, REG_A};

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
    pub fn decodeCB(op: Byte) -> InstructionCB {
        let _ROT_y = ROT[y(op) as usize];
        let _R_z = R_ID[z(op) as usize];
        let _y = y(op);
        match x(op) {
            0 => InstructionCB 
            {
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

    pub const fn fl_z(val: Byte) -> Byte
    {
        if val == 0 {crate::types::FL_Z} else {0}
    }

    pub const fn test(val: Byte, bit: Byte) -> Byte 
    {
        (val & (1 << bit)) >> bit
    }
    
    #[test]
    fn test_bit_test()
    {
        let x: Byte = 0b00000101;
        assert_eq!(test(x, 7), 0);
        assert_eq!(test(x, 6), 0);
        assert_eq!(test(x, 5), 0);
        assert_eq!(test(x, 4), 0);
        assert_eq!(test(x, 3), 0);
        assert_eq!(test(x, 2), 1);
        assert_eq!(test(x, 1), 0);
        assert_eq!(test(x, 0), 1);
    }

    // can't be const for some reason https://github.com/rust-lang/rust/issues/53605
    pub fn signed(val: Byte) -> SByte {
        unsafe { std::mem::transmute(val) }
    }
}
