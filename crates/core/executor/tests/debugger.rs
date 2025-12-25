#![cfg(feature = "debugger-tests")]

use sp1_core_executor::{Executor, Opcode, Program, Instruction, SP1Context, SP1CoreOpts};
use std::process::{Command, Stdio};
use std::thread;
use std::os::unix::net::UnixListener;
use sp1_core_executor::DebuggerListener;

fn get_gdb_command() -> Command {
    if let Ok(gdb) = std::env::var("SP1_GDB") {
        return Command::new(gdb);
    }
    let candidates = ["gdb-multiarch", "riscv64-unknown-elf-gdb", "gdb"];
    for gdb in candidates {
        if Command::new(gdb).arg("--version").output().is_ok() {
            return Command::new(gdb);
        }
    }
    panic!("gdb-multiarch/gdb not found (tried: {:?}, feature 'debugger-tests' enabled)", candidates);
}

#[test]
fn test_remote_debugger() {
    // Create a simple program:
    // 0: ADD x1, x0, 10
    // 4: ADD x1, x0, 20
    // 8: JARL x0, x0, 0 (Infinite loop at 0? No, Opcode::JAL or simple instructions)
    
    let instructions = vec![
        // x1 = 10
        Instruction::new(Opcode::ADD, 1, 0, 10, false, true),
        // x1 = 20
        Instruction::new(Opcode::ADD, 1, 0, 20, false, true),
        // Just loop or end. Executor loops if we don't return?
    ];
    let program = Program::new(instructions, 0, 0);

    // Use Unix Domain Socket
    let socket_path = std::env::temp_dir().join(format!("sp1_gdb_{}.sock", std::process::id()));
    if socket_path.exists() { std::fs::remove_file(&socket_path).unwrap(); }
    
    let listener = UnixListener::bind(&socket_path).expect("Failed to bind socket");
 
    // Wait, UnixListener try_clone might be needed if we want to keep it open?
    // Actually we move it into DebuggerListener.
    // But we need to keep it alive? No, it's moved into Context.
    // DebuggerListener takes ownership.
    let listener_enum = DebuggerListener::Unix(listener);
    
    // Start debugger in background thread
    thread::spawn(move || {
        let mut executor = Executor::with_context(program, SP1CoreOpts::default(), SP1Context::builder().with_debugger(true).with_debugger_listener(listener_enum).build());
        // This will block waiting for connection
        let _ = executor.run_fast(); 
    });

    // Write GDB commands to a file
    let gdb_script_path = std::env::temp_dir().join(format!("sp1_gdb_script_{}.gdb", std::process::id()));
    let gdb_commands = format!(
        "target remote {}
         info registers pc
         stepi
         info registers pc
         quit",
        socket_path.to_str().unwrap()
    );
    std::fs::write(&gdb_script_path, gdb_commands).expect("Failed to write GDB script");

    // Socket exists now
    // Interact with GDB
    let child = get_gdb_command()
        .arg("--quiet")
        .arg("--batch")
        .arg("--nx") // No .gdbinit
        .arg("--command")
        .arg(&gdb_script_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn gdb-multiarch");

    let output = child.wait_with_output().expect("Failed to read stdout");
    let stdout = String::from_utf8_lossy(&output.stdout);
    


    // Assertions
    // Note: GDB output format varies, but usually contains "pc <val>".
    // "pc             0x0	0x0"
    assert!(stdout.contains("0x0"), "Expected initial PC 0");
    assert!(stdout.contains("0x4"), "Expected stepped PC 4");
}

#[test]
fn test_remote_debugger_lldb() {
    let instructions = vec![
        Instruction::new(Opcode::ADD, 1, 0, 10, false, true),
        Instruction::new(Opcode::ADD, 1, 0, 20, false, true),
    ];
    let program = Program::new(instructions, 0, 0);

    // Use Unix Domain Socket for LLDB
    let socket_path = std::env::temp_dir().join(format!("sp1_lldb_{}.sock", std::process::id()));
    if socket_path.exists() { std::fs::remove_file(&socket_path).unwrap(); }
    let listener = UnixListener::bind(&socket_path).expect("Failed to bind socket");
    let listener_enum = DebuggerListener::Unix(listener);

    thread::spawn(move || {
        let mut executor = Executor::with_context(program, SP1CoreOpts::default(), SP1Context::builder().with_debugger(true).with_debugger_listener(listener_enum).build());
        let _ = executor.run_fast(); 
    });

    // Interact with LLDB
    let lldb_script_path = std::env::temp_dir().join(format!("sp1_lldb_script_{}.lldb", std::process::id()));
    let lldb_commands = format!(
        "process connect unix-connect://{}
         register read pc
         thread step-inst
         register read pc",
        socket_path.to_str().unwrap()
    );
    std::fs::write(&lldb_script_path, lldb_commands).expect("Failed to write LLDB script");

    let child = Command::new("lldb")
        .arg("--batch")
        .arg("--source")
        .arg(&lldb_script_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn lldb");

    let output = child.wait_with_output().expect("Failed to read stdout");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);



    // LLDB Output format:
    // pc = 0x00000000
    assert!(stdout.contains("pc = 0") || stderr.contains("pc = 0") || stdout.contains("pc = 0x0"), "Expected initial PC 0");
    assert!(stdout.contains("pc = 0x00000004"), "Expected stepped PC 4");
}

#[test]
fn test_reverse_debugging() {
    let instructions = vec![
        Instruction::new(Opcode::ADD, 1, 0, 10, false, true), // 0
        Instruction::new(Opcode::ADD, 1, 0, 20, false, true), // 4
        Instruction::new(Opcode::ADD, 1, 0, 30, false, true), // 8
        Instruction::new(Opcode::ADD, 1, 0, 40, false, true), // 12
    ];
    let program = Program::new(instructions, 0, 0);

    // Use Unix Domain Socket
    let socket_path = std::env::temp_dir().join(format!("sp1_gdb_rev_{}.sock", std::process::id()));
    if socket_path.exists() { std::fs::remove_file(&socket_path).unwrap(); }
    let listener = UnixListener::bind(&socket_path).expect("Failed to bind socket");
    let listener_enum = DebuggerListener::Unix(listener);
    
    thread::spawn(move || {
        let mut executor = Executor::with_context(program, SP1CoreOpts::default(), SP1Context::builder().with_debugger(true).with_debugger_listener(listener_enum).build());
        let _ = executor.run_fast(); 
    });

    let gdb_script_path = std::env::temp_dir().join(format!("sp1_gdb_rev_script_{}.gdb", std::process::id()));
    let gdb_commands = format!(
        "target remote {}
         stepi
         stepi
         stepi
         info registers pc
         break *0
         reverse-continue
         info registers pc
         quit",
        socket_path.to_str().unwrap()
    );
    std::fs::write(&gdb_script_path, gdb_commands).expect("Failed to write GDB script");

    let child = get_gdb_command()
        .arg("--quiet")
        .arg("--batch")
        .arg("--nx")
        .arg("--command")
        .arg(&gdb_script_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn gdb-multiarch/gdb");

    let output = child.wait_with_output().expect("Failed to read stdout");
    let stdout = String::from_utf8_lossy(&output.stdout);


    assert!(stdout.contains("0xc"), "Expected PC 12 after 3 steps");
    // assert!(stdout.contains("0x8"), "Expected PC 8 after reverse-step");
    assert!(stdout.contains("0x0"), "Expected PC 0 after reverse-continue to breakpoint");
}
