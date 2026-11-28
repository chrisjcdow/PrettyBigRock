// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

// Packages and references: Index into a named package changes it, a store copies, RefOf and
// DerefOf, Match, VarPackage, names in packages (docs/design/0028-acpi.md, section 10). Each T
// method returns 0, or the number of the first check that failed.
DefinitionBlock ("packages.aml", "DSDT", 2, "PBR", "PACKAGES", 1)
{
    Name (PKG, Package () { 1, "two", Buffer () { 3 }, Package () { 4, 5 } })
    Name (NUMS, Package () { 10, 20, 30, 40, 50 })
    // A name in a package that is defined after it: a reference, found when used.
    Name (REFS, Package () { DEV1, 7, INT1 })
    Name (INT1, 0x55)
    Device (DEV1)
    {
        Name (_ADR, 0)
    }

    Method (T001, 0)
    {
        If (SizeOf (PKG) != 4) { Return (1) }
        If (DerefOf (PKG [0]) != 1) { Return (2) }
        If (DerefOf (PKG [1]) != "two") { Return (3) }
        If (ObjectType (PKG [3]) != 4) { Return (4) }
        Local0 = DerefOf (PKG [3])
        If (DerefOf (Local0 [1]) != 5) { Return (5) }
        Return (0)
    }

    // Storing through Index changes the named package; a store into a local copies.
    Method (T002, 0)
    {
        Local0 = NUMS
        NUMS [1] = 21
        If (DerefOf (NUMS [1]) != 21) { Return (1) }
        If (DerefOf (Local0 [1]) != 20) { Return (2) }
        Local1 = Index (NUMS, 2)
        If (DerefOf (Local1) != 30) { Return (3) }
        Return (0)
    }

    // RefOf and DerefOf; a method changes its caller's object through a reference argument.
    Method (BUMP, 1)
    {
        Arg0 = DerefOf (Arg0) + 1
    }
    Method (T003, 0, Serialized)
    {
        Local0 = 5
        BUMP (RefOf (Local0))
        If (Local0 != 6) { Return (1) }
        Name (CNT, 1)
        BUMP (RefOf (CNT))
        If (CNT != 2) { Return (2) }
        If (!CondRefOf (\DEV1, Local1)) { Return (3) }
        If (ObjectType (Local1) != 6) { Return (5) }
        If (CondRefOf (\NOPE)) { Return (4) }
        Return (0)
    }

    Method (T004, 0)
    {
        If (Match (NUMS, MEQ, 40, MTR, 0, 0) != 3) { Return (1) }
        If (Match (NUMS, MGT, 25, MLT, 45, 0) != 2) { Return (2) }
        If (Match (NUMS, MEQ, 99, MTR, 0, 0) != Ones) { Return (3) }
        Local0 = 3
        Local1 = Package (Local0) { 1 }
        If (SizeOf (Local1) != 3) { Return (4) }
        Return (0)
    }

    // A name in a package is a reference to the object, even one defined after the package;
    // DerefOf follows it to the object's value (a device has none).
    Method (T005, 0)
    {
        If (ObjectType (REFS [0]) != 6) { Return (1) }
        If (DerefOf (REFS [1]) != 7) { Return (2) }
        If (DerefOf (REFS [2]) != 0x55) { Return (3) }
        If (ObjectType (REFS [2]) != 1) { Return (4) }
        Return (0)
    }
}
