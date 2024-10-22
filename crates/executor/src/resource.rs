use wasmtime::ResourceLimiter;

pub struct MemoryLimiter {
    max_memory_soft: usize,
    max_memory_hard: usize,
    current: usize,
}

impl MemoryLimiter {
    pub const fn new(max_memory_soft: usize, max_memory_hard: usize) -> Self {
        Self {
            max_memory_soft,
            max_memory_hard,
            current: 0,
        }
    }

    pub const fn exceed_soft(&self) -> bool {
        self.current > self.max_memory_soft
    }
}

impl ResourceLimiter for MemoryLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        if desired > self.max_memory_hard {
            return Ok(false);
        }
        self.current = desired;
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
}
