# Benchmark rkyv vs serde

SP1 uses serde to get data into the guest program.  That involves copying.

We can hack it up to use rkyv instead.

Write a little benchmark (perhaps reading in 10k u32 numbers and computing their sum or so) to compare both approaches.

We can look at total VM CPU cycles and at time.

- [ ] Write serde version of the benchmark.
- [ ] Hack up SP1 to get an rkyv version.


See https://github.com/succinctlabs/sp1/issues/724
