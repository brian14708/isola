use wasmtime::ResourceLimiter;

pub struct MemoryLimiter {
    max_memory_hard: usize,
    max_table_elements_hard: usize,
    current: usize,
}

impl MemoryLimiter {
    pub fn new(max_memory_hard: usize) -> Self {
        // The resource table stores host-side handles. Keep this bounded to avoid
        // untrusted guests growing host memory without limit.
        const TABLE_ELEMENT_BUDGET_BYTES: usize = 64;
        const MIN_TABLE_ELEMENTS: usize = 1024;
        let max_table_elements_hard = core::cmp::max(
            max_memory_hard / TABLE_ELEMENT_BUDGET_BYTES,
            MIN_TABLE_ELEMENTS,
        );

        Self {
            max_memory_hard,
            max_table_elements_hard,
            current: 0,
        }
    }

    pub const fn current(&self) -> usize {
        self.current
    }
}

impl ResourceLimiter for MemoryLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.max_memory_hard {
            return Ok(false);
        }
        self.current = desired;
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.max_table_elements_hard {
            return Ok(false);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_limit_is_enforced() {
        let mut limiter = MemoryLimiter::new(1024);
        assert!(limiter.memory_growing(0, 1024, None).expect("memory grow"));
        assert!(
            !limiter
                .memory_growing(1024, 1025, None)
                .expect("memory grow")
        );
    }

    #[test]
    fn table_limit_is_enforced() {
        let mut limiter = MemoryLimiter::new(64 * 1024);
        // With 64-byte budget and a 1024 floor, this is always over the limit.
        assert!(
            !limiter
                .table_growing(0, usize::MAX, None)
                .expect("table grow")
        );
    }
}
