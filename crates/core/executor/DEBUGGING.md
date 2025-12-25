# Debugging SP1 Programs

SP1 supports step-by-step debugging of RISC-V programs using the `gdbstub` protocol. This allows you to use standard debuggers like GDB or LLDB to inspect execution.

## Usage

To use the debugger, you simply need to set the `SP1_DEBUGGER` environment variable when running your SP1 program with the SDK.

### Starting the Debugger

1.  **Set the Environment Variable**: Set `SP1_DEBUGGER=true` when running your program.
2.  **Run Program**: Execute your program using `cargo run`. The executor will pause and wait for a debugger connection on port 9001 (default).

```bash
SP1_DEBUGGER=true cargo run --release -- --execute
```

*Note: Ensure your program invokes `client.execute(...).run()` which triggers the local executor.*

### Connecting GDB

1.  **Start GDB**: Open a new terminal and start `gdb-multiarch` (or `riscv64-unknown-elf-gdb`).
2.  **Connect**: Connect to the waiting executor.

```bash
gdb-multiarch -ex "target remote :9001"
```

### Custom Port

You can specify a custom port using `SP1_DEBUGGER_PORT`:

```bash
SP1_DEBUGGER=true SP1_DEBUGGER_PORT=1234 cargo run --release -- --execute
```

## Using the SDK

If you are using the SP1 SDK directly in your script (e.g., in `script/src/main.rs`), you can enable the debugger programmatically or by exposing a CLI flag.

```rust
use sp1_sdk::{ProverClient, SP1Stdin};

let client = ProverClient::builder().cpu().build();
// ... setup elf and stdin ...

// Enable debugger directly
let (pv, report) = client
    .execute(elf, &stdin)
    .with_debugger(true) // Enable debugger
    .run()
    .unwrap();
```

You can also specify a custom port:

```rust
// ...
    .with_debugger_port(1234)
    .run()
    .unwrap();
```

If you are writing a custom script using the SP1 SDK, you can enable the debugger by calling `run_debugger(port)` on the `Executor`.

```rust
use sp1_core_executor::{Executor, Program};
use sp1_sdk::SP1CoreOpts;

let program = Program::from_elf("path/to/elf")?;
let mut executor = Executor::new(program, SP1CoreOpts::default());

// This will block until a debugger connects and execution finishes.
executor.run_debugger(9001)?;
```
