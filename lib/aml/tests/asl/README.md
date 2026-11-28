# The AML interpreter's small programs

Each `.asl` file is a small ASL program checking one part of the language (note 0028,
section 10); the `.aml` beside it is what ACPICA's compiler makes of it, committed so the tests
run without `iasl`. Every method named `T` and three digits returns 0, or the number of the
first check that failed. `lib/aml`'s `small_programs` test runs them all, and, when ACPICA's
`acpiexec` is installed, checks that ACPICA gives the same answers.

After changing one, compile it again with constant folding off, so the arithmetic is left for
the interpreter to do rather than worked out by the compiler:

```sh
cd lib/aml/tests/asl
iasl -of -p fields fields.asl
```

`iasl` and `acpiexec` are in Ubuntu's `acpica-tools`.
