#![feature(const_for)]

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
        pub mnemonic: &'static str,
        pub length: u8 // bytes to read
    }
}

pub mod decode {
    use crate::types::{Byte, Instruction};
    use const_format::formatcp;
    use unroll::unroll_for_loops;

    // https://gb-archive.github.io/salvage/decoding_gbz80_opcodes/Decoding%20Gamboy%20Z80%20Opcodes.html

    // arg tables
    const cc: [&'static str; 4] = ["NZ", "Z", "NC", "C"];

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

    #[inline(always)]
    fn x(op: Byte) -> Byte {
        op >> 6
    }
    #[inline(always)]
    fn y(op: Byte) -> Byte {
        op >> 3 & 0b111
    }
    #[inline(always)]
    fn z(op: Byte) -> Byte {
        op & 0b111
    }
    #[inline(always)]
    fn p(op: Byte) -> Byte {
        y(op) >> 1
    }
    #[inline(always)]
    fn q(op: Byte) -> Byte {
        y(op) & 0b1
    }

    #[unroll_for_loops]
    const fn generate_table() -> [Instruction; 0xFF] {
        {
            let mut result = [Instruction{mnemonic: "INVALID", length: 0}; 0xFF];
            let mut i: Byte = 0;
            for i in 0x00..0xFF {
                result[i] = match x(i) {
                    0 => match z(i) {
                        0 => match y(i) {
                            0 => Instruction{mnemonic: "NOP", length: 1},
                            1 => Instruction{mnemonic: "LD (nn), SP", length: 3},
                            2 => Instruction{mnemonic: "STOP", length: 1},
                            3 => Instruction{mnemonic: "JR d", length: 2},
                            4..=7 => {
                                const idx: usize = (y(i) - 4) as usize;
                                const flag: &'static str = cc[idx];
                                const mnem: &'static str = formatcp!("JR {flag}, d");
                                Instruction{mnemonic: &mnem, length: 2}
                            }
                            _ => Instruction{mnemonic: "INVALID", length: 0}
                        }
                        1_u8..=u8::MAX => todo!()
                    }
                    1_u8..=u8::MAX => todo!()
                }
            }
            result
        }
    }

    const INSTRUCTION_TABLE: [Instruction; 0xFF] = generate_table();

    pub const fn decode(op: Byte) -> Instruction {
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
