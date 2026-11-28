// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

// Integers, strings and buffers: arithmetic, buffer fields, the conversions AML makes without
// being asked and the ones it is asked for (docs/design/0028-acpi.md, section 10). Each T method
// returns 0, or the number of the first check that failed.
DefinitionBlock ("data.aml", "DSDT", 2, "PBR", "DATA", 1)
{
    Name (BUF, Buffer (8) { 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08 })
    Name (INT, 0x1234)
    Name (STR, "Hello")

    // Arithmetic wraps; division gives both answers; the bit operators.
    Method (T001, 0)
    {
        If ((0xFFFFFFFFFFFFFFFF + 2) != 1) { Return (1) }
        Divide (17, 5, Local0, Local1)
        If ((Local0 != 2) || (Local1 != 3)) { Return (2) }
        If ((7 % 4) != 3) { Return (3) }
        If ((1 << 63) != 0x8000000000000000) { Return (4) }
        If ((0xF0 >> 4) != 0x0F) { Return (5) }
        If ((~0) != Ones) { Return (6) }
        If (NAnd (0xFF, 0x0F) != 0xFFFFFFFFFFFFFFF0) { Return (7) }
        If (FindSetLeftBit (0x80) != 8) { Return (8) }
        If (FindSetRightBit (0x80) != 8) { Return (9) }
        If (FindSetLeftBit (0) != 0) { Return (10) }
        If (FromBCD (0x1234) != 1234) { Return (11) }
        If (ToBCD (1234) != 0x1234) { Return (12) }
        Local2 = 5
        Local2++
        Local2 -= 2
        If (Local2 != 4) { Return (13) }
        If (!(1 && 2) || (0 || 0)) { Return (14) }
        Return (0)
    }

    // Buffer fields read and write bits of a named buffer.
    Method (T002, 0)
    {
        CreateByteField (BUF, 1, BYT1)
        CreateWordField (BUF, 2, WRD1)
        CreateDWordField (BUF, 0, DWD0)
        CreateQWordField (BUF, 0, QWD0)
        CreateBitField (BUF, 0, BIT0)
        CreateField (BUF, 12, 8, FLD)
        If (BYT1 != 0x02) { Return (1) }
        If (WRD1 != 0x0403) { Return (2) }
        If (DWD0 != 0x04030201) { Return (3) }
        If (QWD0 != 0x0807060504030201) { Return (4) }
        If (BIT0 != 1) { Return (5) }
        // Bits 12 to 19: the top half of 0x02 and the bottom half of 0x03. A field made by
        // CreateField reads as a buffer, whatever its length (as ACPICA and Windows have it).
        If (FLD != Buffer () { 0x30 }) { Return (6) }
        WRD1 = 0xBEEF
        If (DerefOf (BUF [2]) != 0xEF) { Return (7) }
        FLD = 0xFF
        If (DerefOf (BUF [1]) != 0xF2) { Return (8) }
        Return (0)
    }

    // Comparisons go by the first operand's kind; Concatenate's result is the first's kind.
    Method (T003, 0)
    {
        If ("ABC" != "ABC") { Return (1) }
        If (!("ABC" < "ABD")) { Return (2) }
        If (Buffer () { 1, 2 } != Buffer () { 1, 2 }) { Return (3) }
        // A string compared with an integer: the integer becomes hexadecimal text.
        Local0 = Concatenate ("x", 0x1A)
        If (Local0 != "x000000000000001A") { Return (4) }
        Local1 = Concatenate (Buffer () { 1 }, Buffer () { 2, 3 })
        If (SizeOf (Local1) != 3) { Return (5) }
        Local2 = Concatenate (1, 2)
        If (SizeOf (Local2) != 16) { Return (6) }
        // A string's leading hexadecimal digits as an integer.
        If (("1F" + 1) != 0x20) { Return (7) }
        Return (0)
    }

    // The conversions asked for.
    Method (T004, 0)
    {
        If (ToInteger ("0x1F") != 31) { Return (1) }
        If (ToInteger ("123") != 123) { Return (2) }
        If (ToDecimalString (123) != "123") { Return (3) }
        If (ToHexString (0x1A) != "0x1A") { Return (4) }
        If (ToHexString (Buffer () { 1, 0xAB }) != "0x01,0xAB") { Return (9) }
        If (ToDecimalString (Buffer () { 1, 0xAB }) != "1,171") { Return (10) }
        If (Concatenate ("x", Buffer () { 1, 0xAB }) != "x0x01 0xAB") { Return (11) }
        Local0 = ToBuffer ("AB")
        If (SizeOf (Local0) != 3) { Return (5) }
        If (ToString (Buffer () { 0x41, 0x42, 0x00, 0x43 }, Ones) != "AB") { Return (6) }
        If (Mid ("Hello", 1, 3) != "ell") { Return (7) }
        Local1 = Mid (BUF, 6, 10)
        If (SizeOf (Local1) != 2) { Return (8) }
        Return (0)
    }

    // A store to a named object converts to the kind it holds; a buffer keeps its length.
    Method (T005, 0, Serialized)
    {
        INT = "42"
        If (INT != 0x42) { Return (1) }
        STR = 0x1F
        If (STR != "000000000000001F") { Return (2) }
        Name (SMAL, Buffer (2) { 0, 0 })
        SMAL = 0x11223344
        If (SizeOf (SMAL) != 2) { Return (3) }
        If (DerefOf (SMAL [0]) != 0x44) { Return (4) }
        // CopyObject replaces it instead.
        CopyObject ("now a string", INT)
        If (ObjectType (INT) != 2) { Return (5) }
        Return (0)
    }
}
