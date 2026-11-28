// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

// Control: While with Break and Continue, If and Else, Switch (which iasl turns into While and
// If), recursion, a Serialized method's names, Return of a package, mutexes and events
// (docs/design/0028-acpi.md, section 10). Each T method returns 0, or the number of the first
// check that failed.
DefinitionBlock ("control.aml", "DSDT", 2, "PBR", "CONTROL", 1)
{
    Mutex (MTX, 0)
    Event (EVT)

    Method (T001, 0)
    {
        Local0 = 0
        Local1 = 0
        While (One)
        {
            Local0++
            If (Local0 > 10) { Break }
            If ((Local0 & 1) == 0) { Continue }
            Local1 += Local0
        }
        // 1 + 3 + 5 + 7 + 9
        If (Local1 != 25) { Return (1) }
        If (Local0 != 11) { Return (2) }
        Return (0)
    }

    Method (PICK, 1)
    {
        Switch (ToInteger (Arg0))
        {
            Case (1) { Return ("one") }
            Case (Package () { 2, 3 }) { Return ("two or three") }
            Default { Return ("many") }
        }
    }
    Method (T002, 0)
    {
        If (PICK (1) != "one") { Return (1) }
        If (PICK (3) != "two or three") { Return (2) }
        If (PICK (9) != "many") { Return (3) }
        If (1) { Local0 = 1 } Else { Local0 = 2 }
        If (0) { Local1 = 1 } ElseIf (1) { Local1 = 2 } Else { Local1 = 3 }
        If ((Local0 != 1) || (Local1 != 2)) { Return (4) }
        Return (0)
    }

    // Recursion, each call with its own locals.
    Method (FACT, 1)
    {
        If (Arg0 <= 1) { Return (1) }
        Local0 = FACT (Arg0 - 1)
        Return (Arg0 * Local0)
    }
    // A Serialized method's name goes when it returns, so a second call can make it again.
    Method (MKNM, 1, Serialized)
    {
        Name (MINE, 0)
        MINE = Arg0
        Return (MINE + 1)
    }
    Method (T003, 0)
    {
        If (FACT (10) != 3628800) { Return (1) }
        If (MKNM (4) != 5) { Return (2) }
        If (MKNM (7) != 8) { Return (3) }
        Return (0)
    }

    Method (PAIR, 0)
    {
        Return (Package () { "a", 2 })
    }
    Method (T004, 0)
    {
        Local0 = PAIR ()
        If (SizeOf (Local0) != 2) { Return (1) }
        If (DerefOf (Local0 [0]) != "a") { Return (2) }
        Return (0)
    }

    // A mutex taken twice is let go twice; an event signalled is waited for at once.
    Method (T005, 0)
    {
        If (Acquire (MTX, 0xFFFF)) { Return (1) }
        If (Acquire (MTX, 0)) { Return (2) }
        Release (MTX)
        Release (MTX)
        Signal (EVT)
        If (Wait (EVT, 100)) { Return (3) }
        Reset (EVT)
        If (!Wait (EVT, 1)) { Return (4) }
        Return (0)
    }

    // \_OSI says yes to Windows and no to anything else; \_REV and \_OS are there.
    Method (T006, 0)
    {
        If (!\_OSI ("Windows 2009")) { Return (1) }
        If (\_OSI ("Linux")) { Return (2) }
        If (\_OSI ("PrettyBigRock")) { Return (3) }
        If (\_REV != 2) { Return (4) }
        If (ObjectType (\_OS) != 2) { Return (5) }
        Return (0)
    }
}
