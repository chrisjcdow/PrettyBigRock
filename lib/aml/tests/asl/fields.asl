// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

// Fields of every access type, the three update rules, index fields and bank fields
// (docs/design/0028-acpi.md, section 10). Each T method returns 0, or the number of the first
// check that failed. The memory is the tests' blank machine: it reads back what was written.
DefinitionBlock ("fields.aml", "DSDT", 2, "PBR", "FIELDS", 1)
{
    OperationRegion (MEM, SystemMemory, 0x10000, 0x40)
    Field (MEM, ByteAcc, NoLock, Preserve)
    {
        B0, 8, B1, 8, B2, 8, B3, 8,
        Offset (8), Q0, 64
    }
    Field (MEM, WordAcc, NoLock, Preserve)
    {
        Offset (0x10), W0, 4, W1, 8, W2, 4
    }
    Field (MEM, DWordAcc, NoLock, WriteAsOnes)
    {
        Offset (0x14), D0, 8, , 16, D1, 8
    }
    Field (MEM, DWordAcc, NoLock, WriteAsZeros)
    {
        Offset (0x18), Z0, 8
    }
    Field (MEM, QWordAcc, NoLock, Preserve)
    {
        Offset (0x20), X0, 12, X1, 40
    }
    Field (MEM, AnyAcc, NoLock, Preserve)
    {
        Offset (0x28), A0, 3, A1, 13
    }
    Field (MEM, ByteAcc, NoLock, Preserve)
    {
        Offset (0x30), BIG, 96
    }
    // Bytes 0x10 to 0x1B one at a time, to see what the others wrote.
    Field (MEM, ByteAcc, NoLock, Preserve)
    {
        Offset (0x10), R10, 8, R11, 8, R12, 8, R13, 8, R14, 8, R15, 8, R16, 8, R17, 8,
        R18, 8, R19, 8, R1A, 8, R1B, 8
    }

    // Index and data registers in memory: the index field writes the datum's offset to IDX,
    // then reads or writes DAT.
    OperationRegion (IDR, SystemMemory, 0x20000, 2)
    Field (IDR, ByteAcc, NoLock, Preserve) { IDX, 8, DAT, 8 }
    IndexField (IDX, DAT, ByteAcc, NoLock, Preserve)
    {
        Offset (2), I0, 8, I1, 4
    }

    // A bank field: BNK chooses which bank the region shows.
    OperationRegion (BKR, SystemMemory, 0x30000, 4)
    Field (BKR, ByteAcc, NoLock, Preserve) { BNK, 8 }
    BankField (BKR, BNK, 3, ByteAcc, NoLock, Preserve)
    {
        Offset (1), K3, 8
    }

    Method (T001, 0)
    {
        B0 = 0x11
        B1 = 0x22
        B2 = 0x33
        B3 = 0x44
        If (B2 != 0x33) { Return (1) }
        Q0 = 0x0102030405060708
        If (Q0 != 0x0102030405060708) { Return (2) }
        Return (0)
    }

    // Word access, fields inside one word: writing W1 keeps W0 and W2 (Preserve).
    Method (T002, 0)
    {
        W0 = 0xA
        W2 = 0x5
        W1 = 0xBC
        If (W0 != 0xA) { Return (1) }
        If (W1 != 0xBC) { Return (2) }
        If (W2 != 0x5) { Return (3) }
        // In memory: bits 0-3 W0, 4-11 W1, 12-15 W2: 0x5BCA.
        If (R10 != 0xCA) { Return (4) }
        If (R11 != 0x5B) { Return (5) }
        Return (0)
    }

    // WriteAsOnes and WriteAsZeros fill the rest of the double word.
    Method (T003, 0)
    {
        D0 = 0x12
        If (D0 != 0x12) { Return (1) }
        If (D1 != 0xFF) { Return (2) }
        // The 16 bits skipped between D0 and D1 were written as ones too.
        If (R15 != 0xFF) { Return (3) }
        Z0 = 0x34
        If (Z0 != 0x34) { Return (4) }
        If (R19 != 0) { Return (5) }
        Return (0)
    }

    // A quad-word field across a byte boundary, and one over 32 bits.
    Method (T004, 0)
    {
        X0 = 0xABC
        X1 = 0x123456789A
        If (X0 != 0xABC) { Return (1) }
        If (X1 != 0x123456789A) { Return (2) }
        A0 = 5
        A1 = 0x1234
        If (A0 != 5) { Return (3) }
        If (A1 != 0x1234) { Return (4) }
        Return (0)
    }

    // A field longer than an integer reads as a buffer.
    Method (T005, 0)
    {
        BIG = Buffer () { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12 }
        Local0 = BIG
        If (ObjectType (Local0) != 3) { Return (1) }
        If (SizeOf (Local0) != 12) { Return (2) }
        If (DerefOf (Local0 [11]) != 12) { Return (3) }
        Return (0)
    }

    // An index field: the index register is given the datum's byte offset.
    Method (T006, 0)
    {
        I0 = 0x5A
        If (IDX != 2) { Return (1) }
        If (DAT != 0x5A) { Return (2) }
        I1 = 0xF
        If (IDX != 3) { Return (3) }
        Return (0)
    }

    // A bank field writes its bank number first.
    Method (T007, 0)
    {
        BNK = 0
        K3 = 0x77
        If (BNK != 3) { Return (1) }
        Return (0)
    }
}
