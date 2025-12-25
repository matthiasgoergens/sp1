use gdbstub::{
    arch::Arch,
    common::Signal,
    stub::BaseStopReason,
    target::{
        ext::{
            base::{
                reverse_exec::{ReverseCont, ReverseContOps, ReverseStep, ReverseStepOps},
                singlethread::{
                    SingleThreadBase, SingleThreadResume, SingleThreadResumeOps,
                    SingleThreadSingleStep, SingleThreadSingleStepOps,
                },
                BaseOps,
            },
            breakpoints::{
                Breakpoints, BreakpointsOps, HwWatchpoint, HwWatchpointOps, SwBreakpoint,
                SwBreakpointOps, WatchKind,
            },
            catch_syscalls::{
                CatchSyscallPosition, CatchSyscalls, CatchSyscallsOps, SyscallNumbers,
            },
            lldb_register_info_override::{
                Callback, LldbRegisterInfoOverride, LldbRegisterInfoOverrideOps,
            },
            memory_map::{MemoryMap, MemoryMapOps},
            target_description_xml_override::{
                TargetDescriptionXmlOverride, TargetDescriptionXmlOverrideOps,
            },
        },
        Target, TargetResult,
    },
};
use gdbstub_arch::riscv::{reg::RiscvCoreRegs, Riscv32};

use crate::{
    events::MemoryRecord,
    executor::{ExecutionDirection, Executor, MemoryAccessType},
    state::ExecutionState,
    syscalls::SyscallCode,
};
use hashbrown::{HashMap, HashSet};
use std::collections::VecDeque;

/// The state of the debugger.
#[derive(Debug, Default)]
pub struct DebuggerState {
    pub(crate) execution_direction: ExecutionDirection,
    pub(crate) breakpoints: HashSet<u32>,
    pub(crate) watchpoints: HashMap<u32, gdbstub::target::ext::breakpoints::WatchKind>,
    pub(crate) last_syscall: Option<SyscallCode>,
    pub(crate) history: VecDeque<ExecutionState>,
    pub(crate) last_access_addr: Option<u32>,
    pub(crate) last_access_type: Option<MemoryAccessType>,
    pub(crate) catch_syscalls: HashSet<u32>,
    pub(crate) catch_syscalls_enabled: bool,
    pub(crate) at_syscall_entry: bool,
    pub(crate) is_stepping: bool,
}


impl Target for Executor<'_> {
    type Arch = Riscv32;
    type Error = &'static str;

    fn base_ops(&mut self) -> BaseOps<'_, Self::Arch, Self::Error> {
        BaseOps::SingleThread(self)
    }

    fn support_breakpoints(&mut self) -> Option<BreakpointsOps<'_, Self>> {
        Some(self)
    }

    fn support_catch_syscalls(&mut self) -> Option<CatchSyscallsOps<'_, Self>> {
        Some(self)
    }

    fn support_memory_map(&mut self) -> Option<MemoryMapOps<'_, Self>> {
        Some(self)
    }

    fn support_target_description_xml_override(
        &mut self,
    ) -> Option<TargetDescriptionXmlOverrideOps<'_, Self>> {
        Some(self)
    }

    fn support_lldb_register_info_override(
        &mut self,
    ) -> Option<LldbRegisterInfoOverrideOps<'_, Self>> {
        Some(self)
    }
}

impl SingleThreadBase for Executor<'_> {
    fn read_registers(&mut self, regs: &mut RiscvCoreRegs<u32>) -> TargetResult<(), Self> {
        for i in 0..32 {
            regs.x[i] = self.state.memory.registers.get(i as u32).map_or(0, |r| r.value);
        }
        regs.pc = self.state.pc;
        Ok(())
    }

    fn write_registers(&mut self, regs: &RiscvCoreRegs<u32>) -> TargetResult<(), Self> {
        for i in 0..32 {
            let record = MemoryRecord {
                value: regs.x[i],
                shard: self.state.current_shard,
                timestamp: self.state.clk,
            };
            self.state.memory.registers.insert(i as u32, record);
        }
        self.state.pc = regs.pc;
        Ok(())
    }

    fn read_addrs(&mut self, start_addr: u32, data: &mut [u8]) -> TargetResult<usize, Self> {
        for (i, byte) in data.iter_mut().enumerate() {
            let addr = start_addr + i as u32;
            let word_addr = addr & !3;
            let word = self.state.memory.get(word_addr).map_or(0, |r| r.value);
            let byte_offset = (addr % 4) * 8;
            *byte = ((word >> byte_offset) & 0xff) as u8;
        }
        Ok(data.len())
    }

    fn write_addrs(&mut self, start_addr: u32, data: &[u8]) -> TargetResult<(), Self> {
        for (i, &byte) in data.iter().enumerate() {
            let addr = start_addr + i as u32;
            let word_addr = addr & !3;
            let mut word = self.state.memory.get(word_addr).map_or(0, |r| r.value);
            let byte_offset = (addr % 4) * 8;

            word &= !(0xff << byte_offset);
            word |= (byte as u32) << byte_offset;

            let record = MemoryRecord {
                value: word,
                shard: self.state.current_shard,
                timestamp: self.state.clk,
            };
            self.state.memory.insert(word_addr, record);
        }
        Ok(())
    }

    fn support_resume(&mut self) -> Option<SingleThreadResumeOps<'_, Self>> {
        Some(self)
    }
}

impl SingleThreadResume for Executor<'_> {
    fn resume(&mut self, _signal: Option<Signal>) -> Result<(), <Self as Target>::Error> {
        if let Some(state) = &mut self.debugger_state {
            state.execution_direction = ExecutionDirection::Forward;
            state.is_stepping = false;
        }
        Ok(())
    }

    fn support_single_step(&mut self) -> Option<SingleThreadSingleStepOps<'_, Self>> {
        Some(self)
    }

    fn support_reverse_step(&mut self) -> Option<ReverseStepOps<'_, (), Self>> {
        Some(self)
    }

    fn support_reverse_cont(&mut self) -> Option<ReverseContOps<'_, (), Self>> {
        Some(self)
    }
}

impl SingleThreadSingleStep for Executor<'_> {
     fn step(&mut self, _signal: Option<Signal>) -> Result<(), <Self as Target>::Error> {
        if let Some(state) = &mut self.debugger_state {
            state.execution_direction = ExecutionDirection::Forward;
            state.is_stepping = true;
        }
        Ok(())
    }
}

impl ReverseStep<()> for Executor<'_> {
    fn reverse_step(&mut self, _tid: ()) -> Result<(), <Self as Target>::Error> {
        let history = &mut self.debugger_state.as_mut().unwrap().history;
        if let Some(prev_state) = history.pop_back() {
            self.state = prev_state;
            self.debugger_state.as_mut().unwrap().is_stepping = true;
            Ok(())
        } else {
            Err("no history available")
        }
    }
}

impl ReverseCont<()> for Executor<'_> {
    fn reverse_cont(&mut self) -> Result<(), <Self as Target>::Error> {
        if let Some(state) = &mut self.debugger_state {
            state.execution_direction = ExecutionDirection::Backward;
            state.is_stepping = false;
        }
        Ok(())
    }
}

impl SwBreakpoint for Executor<'_> {
    fn add_sw_breakpoint(
        &mut self,
        addr: u32,
        _kind: <Self::Arch as Arch>::BreakpointKind,
    ) -> TargetResult<bool, Self> {
        self.debugger_state.as_mut().unwrap().breakpoints.insert(addr);
        Ok(true)
    }

    fn remove_sw_breakpoint(
        &mut self,
        addr: u32,
        _kind: <Self::Arch as Arch>::BreakpointKind,
    ) -> TargetResult<bool, Self> {
        self.debugger_state.as_mut().unwrap().breakpoints.remove(&addr);
        Ok(true)
    }
}

impl HwWatchpoint for Executor<'_> {
    fn add_hw_watchpoint(
        &mut self,
        addr: u32,
        _len: u32,
        kind: WatchKind,
    ) -> TargetResult<bool, Self> {
        self.debugger_state.as_mut().unwrap().watchpoints.insert(addr, kind);
        Ok(true)
    }

    fn remove_hw_watchpoint(
        &mut self,
        addr: u32,
        _len: u32,
        _kind: WatchKind,
    ) -> TargetResult<bool, Self> {
        self.debugger_state.as_mut().unwrap().watchpoints.remove(&addr);
        Ok(true)
    }
}

impl Breakpoints for Executor<'_> {
    fn support_sw_breakpoint(&mut self) -> Option<SwBreakpointOps<'_, Self>> {
        Some(self)
    }

    fn support_hw_watchpoint(&mut self) -> Option<HwWatchpointOps<'_, Self>> {
        Some(self)
    }
}

impl CatchSyscalls for Executor<'_> {
    fn enable_catch_syscalls(
        &mut self,
        filter: Option<SyscallNumbers<'_, <Self::Arch as Arch>::Usize>>,
    ) -> TargetResult<(), Self> {
        let state = self.debugger_state.as_mut().unwrap();
        state.catch_syscalls_enabled = true;
        state.catch_syscalls.clear();
        if let Some(numbers) = filter {
            for num in numbers {
                state.catch_syscalls.insert(num);
            }
        }
        Ok(())
    }

    fn disable_catch_syscalls(&mut self) -> TargetResult<(), Self> {
        let state = self.debugger_state.as_mut().unwrap();
        state.catch_syscalls_enabled = false;
        state.catch_syscalls.clear();
        Ok(())
    }
}

impl MemoryMap for Executor<'_> {
    fn memory_map_xml(
        &self,
        offset: u64,
        length: usize,
        buf: &mut [u8],
    ) -> TargetResult<usize, Self> {
        // SP1 has a flat memory space. We'll report a single large RAM region.
        let xml = r#"<memory-map>
  <memory type="ram" start="0x0" length="0xffffffff"/>
</memory-map>"#;
        let xml_bytes = xml.as_bytes();
        if offset >= xml_bytes.len() as u64 {
            return Ok(0);
        }
        let start = offset as usize;
        let end = (start + length).min(xml_bytes.len());
        let len = end - start;
        buf[..len].copy_from_slice(&xml_bytes[start..end]);
        Ok(len)
    }
}

impl TargetDescriptionXmlOverride for Executor<'_> {
    fn target_description_xml(
        &self,
        annex: &[u8],
        offset: u64,
        length: usize,
        buf: &mut [u8],
    ) -> TargetResult<usize, Self> {
        if annex != b"target.xml" {
            return Ok(0);
        }

        let xml = r#"<?xml version="1.0"?>
<target version="1.0">
  <architecture>riscv:rv32</architecture>
  <feature name="org.gnu.gdb.riscv.cpu">
    <reg name="zero" bitsize="32" type="int" regnum="0"/>
    <reg name="ra" bitsize="32" type="code_ptr"/>
    <reg name="sp" bitsize="32" type="data_ptr"/>
    <reg name="gp" bitsize="32" type="data_ptr"/>
    <reg name="tp" bitsize="32" type="data_ptr"/>
    <reg name="t0" bitsize="32" type="int"/>
    <reg name="t1" bitsize="32" type="int"/>
    <reg name="t2" bitsize="32" type="int"/>
    <reg name="fp" bitsize="32" type="data_ptr"/>
    <reg name="s1" bitsize="32" type="int"/>
    <reg name="a0" bitsize="32" type="int"/>
    <reg name="a1" bitsize="32" type="int"/>
    <reg name="a2" bitsize="32" type="int"/>
    <reg name="a3" bitsize="32" type="int"/>
    <reg name="a4" bitsize="32" type="int"/>
    <reg name="a5" bitsize="32" type="int"/>
    <reg name="a6" bitsize="32" type="int"/>
    <reg name="a7" bitsize="32" type="int"/>
    <reg name="s2" bitsize="32" type="int"/>
    <reg name="s3" bitsize="32" type="int"/>
    <reg name="s4" bitsize="32" type="int"/>
    <reg name="s5" bitsize="32" type="int"/>
    <reg name="s6" bitsize="32" type="int"/>
    <reg name="s7" bitsize="32" type="int"/>
    <reg name="s8" bitsize="32" type="int"/>
    <reg name="s9" bitsize="32" type="int"/>
    <reg name="s10" bitsize="32" type="int"/>
    <reg name="s11" bitsize="32" type="int"/>
    <reg name="t3" bitsize="32" type="int"/>
    <reg name="t4" bitsize="32" type="int"/>
    <reg name="t5" bitsize="32" type="int"/>
    <reg name="t6" bitsize="32" type="int"/>
    <reg name="pc" bitsize="32" type="code_ptr"/>
  </feature>
</target>"#;
        let xml_bytes = xml.as_bytes();
        if offset >= xml_bytes.len() as u64 {
            return Ok(0);
        }
        let start = offset as usize;
        let end = (start + length).min(xml_bytes.len());
        let len = end - start;
        buf[..len].copy_from_slice(&xml_bytes[start..end]);
        Ok(len)
    }
}

impl LldbRegisterInfoOverride for Executor<'_> {
    fn lldb_register_info<'a>(
        &mut self,
        reg_id: usize,
        reg_info: Callback<'a>,
    ) -> Result<gdbstub::target::ext::lldb_register_info_override::CallbackToken<'a>, Self::Error>
    {
        use gdbstub::arch::lldb::{Encoding, Format, Register};
        let regs = [
            "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "fp", "s1", "a0", "a1", "a2", "a3",
            "a4", "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11",
            "t3", "t4", "t5", "t6", "pc",
        ];

        if reg_id >= regs.len() {
            return Ok(reg_info.done());
        }

        let name = regs[reg_id];
        let reg = Register {
            name,
            alt_name: None,
            bitsize: 32,
            offset: reg_id * 4,
            encoding: Encoding::Uint,
            format: Format::Hex,
            set: "General Purpose Registers",
            gcc: Some(reg_id),
            dwarf: Some(reg_id),
            generic: match name {
                "pc" => Some(gdbstub::arch::lldb::Generic::Pc),
                "sp" => Some(gdbstub::arch::lldb::Generic::Sp),
                "fp" => Some(gdbstub::arch::lldb::Generic::Fp),
                "ra" => Some(gdbstub::arch::lldb::Generic::Ra),
                _ => None,
            },
            container_regs: None,
            invalidate_regs: None,
        };

        Ok(reg_info.write(reg))
    }
}

/// A custom event loop for the debugger.
pub struct DebuggerEventLoop<'a>(std::marker::PhantomData<&'a ()>);

impl<'a> gdbstub::stub::run_blocking::BlockingEventLoop for DebuggerEventLoop<'a> {
    type Target = Executor<'a>;
    type Connection = std::net::TcpStream;
    type StopReason = BaseStopReason<(), u32>;

    fn wait_for_stop_reason(
        target: &mut Self::Target,
        conn: &mut Self::Connection,
    ) -> Result<
        gdbstub::stub::run_blocking::Event<Self::StopReason>,
        gdbstub::stub::run_blocking::WaitForStopReasonError<
            <Self::Target as Target>::Error,
            std::io::Error,
        >,
    > {
        loop {
            // Check for GDB interrupt every 1024 cycles to avoid blocking overhead.
            if target.state.clk % 1024 == 0 {
                conn.set_nonblocking(true).ok();
                let mut buf = [0u8; 1];
                let peek_res = conn.peek(&mut buf);
                conn.set_nonblocking(false).ok();

                if let Ok(1) = peek_res {
                    if buf[0] == 0x03 {
                         return Ok(gdbstub::stub::run_blocking::Event::TargetStopped(
                            BaseStopReason::Signal(Signal::SIGINT),
                        ));
                    }
                }
            }


            // If we are currently at a syscall entry, execute the syscall and stop at Return.
            if target.debugger_state.as_mut().unwrap().at_syscall_entry {
                target.debugger_state.as_mut().unwrap().at_syscall_entry = false;
                // Need to drop borrow to access fields again or use a block?
                // Just accessing sequentially is fine.
                let state = target.debugger_state.as_mut().unwrap();
                if state.catch_syscalls_enabled {
                    if let Some(syscall) = state.last_syscall {
                        let num = syscall as u32;
                        if state.catch_syscalls.is_empty() || state.catch_syscalls.contains(&num) {
                            return Ok(gdbstub::stub::run_blocking::Event::TargetStopped(
                                BaseStopReason::CatchSyscall {
                                    tid: Some(()),
                                    number: num,
                                    position: CatchSyscallPosition::Return,
                                },
                            ));
                        }
                    }
                }
            }

            let direction = target.debugger_state.as_mut().unwrap().execution_direction;
            match direction {
                ExecutionDirection::Forward => {
                    // Save state for reverse debugging if needed.
                    target.debugger_state.as_mut().unwrap().history.push_back(target.state.clone());

                    // Clear last access info before executing.
                    {
                        let state = target.debugger_state.as_mut().unwrap();
                        state.last_access_addr = None;
                        state.last_access_type = None;
                        state.last_syscall = None;
                    }

                    let done = target.execute_cycle().map_err(|_| {
                        gdbstub::stub::run_blocking::WaitForStopReasonError::Target("execution failed")
                    })?;

                    if done {
                        return Ok(gdbstub::stub::run_blocking::Event::TargetStopped(
                            BaseStopReason::Exited(0),
                        ));
                    }
                }
                ExecutionDirection::Backward => {
                    // Pop history to go back.
                    let prev_state = target.debugger_state.as_mut().unwrap().history.pop_back();
                    if let Some(prev) = prev_state {
                        target.state = prev;
                        // In reverse, we don't have new memory accesses or syscalls to report
                        // in the same way, so we clear them to avoid false positives.
                        let state = target.debugger_state.as_mut().unwrap();
                        state.last_access_addr = None;
                        state.last_access_type = None;
                        state.last_syscall = None;
                        state.at_syscall_entry = false;
                    } else {
                        return Ok(gdbstub::stub::run_blocking::Event::TargetStopped(
                            BaseStopReason::Exited(0),
                        ));
                    }
                }
            }

            let state = target.debugger_state.as_mut().unwrap();

            // Check for syscalls.
            if state.catch_syscalls_enabled {
                if let Some(syscall) = state.last_syscall {
                    let num = syscall as u32;
                    if state.catch_syscalls.is_empty() || state.catch_syscalls.contains(&num) {
                        state.at_syscall_entry = true;
                        return Ok(gdbstub::stub::run_blocking::Event::TargetStopped(
                            BaseStopReason::CatchSyscall {
                                tid: Some(()),
                                number: num,
                                position: CatchSyscallPosition::Entry,
                            },
                        ));
                    }
                }
            }

            // Check for software breakpoints.
            if state.breakpoints.contains(&target.state.pc) {
                return Ok(gdbstub::stub::run_blocking::Event::TargetStopped(BaseStopReason::SwBreak(
                    (),
                )));
            }

            // Check for watchpoints.
            if let Some(addr) = state.last_access_addr {
                if let Some(kind) = state.watchpoints.get(&addr) {
                    let triggered = matches!(
                        (kind, state.last_access_type),
                        (WatchKind::Write, Some(MemoryAccessType::Write)) |
                            (WatchKind::Read, Some(MemoryAccessType::Read)) |
                            (WatchKind::ReadWrite, Some(_))
                    );

                    if triggered {
                        return Ok(gdbstub::stub::run_blocking::Event::TargetStopped(
                            BaseStopReason::Watch { tid: (), kind: *kind, addr },
                        ));
                    }
                }
            }

            // If stepping, return DoneStep. Otherwise loop.
            if state.is_stepping {
                return Ok(gdbstub::stub::run_blocking::Event::TargetStopped(BaseStopReason::DoneStep));
            }
        }
    }

    fn on_interrupt(
        _target: &mut Self::Target,
    ) -> Result<Option<Self::StopReason>, <Self::Target as Target>::Error> {
        Ok(Some(BaseStopReason::Signal(Signal::SIGINT)))
    }
}
