# Roxy's ACPI tables

The ACPI tables of Roxy, the machine milestone M7 is built for (docs/m7-plan.md): a Foxconn
H61MX board (Intel H61, i5-2300) with American Megatrends firmware `BC1F1P08` of 25 December
2013. They are the test data for the AML interpreter (`lib/aml`, note 0028), so that it is
checked against a real machine's tables on every change, not only QEMU's.

Copied on 23 November 2026 from Linux's `/sys/firmware/acpi/tables`, unchanged:

```sh
sudo tar czf roxy-acpi.tgz -C /sys/firmware/acpi/tables .
```

`dynamic/` holds the SSDTs the firmware's own AML loads at run time (the processors' power
states); the others are the tables the firmware lists. They contain nothing unique to Roxy
(no serial numbers, network addresses or product keys); every H61MX with this firmware has the
same ones.

**These files are not part of PrettyBigRock and are not covered by its licence.** They are
firmware, the work of Foxconn and American Megatrends, kept here only as test data for
interoperability, as collections such as `linuxhw/ACPI` keep many thousands of machines'
tables. If their owners object, they will be removed.
