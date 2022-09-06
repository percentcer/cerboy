pub mod types {
    pub type Byte = u8;
    pub type Word = u16;
    pub type SByte = i8;
    pub type SWord = i16;
pub mod decode {
    use crate::types::Byte;

    // https://gb-archive.github.io/salvage/decoding_gbz80_opcodes/Decoding%20Gamboy%20Z80%20Opcodes.html
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
