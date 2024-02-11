use std::collections::HashMap;

use parking_lot::Mutex;
use rand::Rng;

use crate::vm::Vm;

pub struct VmCache {
    caches: Mutex<HashMap<[u8; 32], Vec<Vm>>>,
}

impl VmCache {
    pub fn new() -> Self {
        Self {
            caches: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, hash: [u8; 32]) -> Option<Vm> {
        let mut caches = self.caches.lock();
        caches.get_mut(&hash)?.pop()
    }

    pub fn put(&self, vm: Vm) {
        if !vm.store.data().reuse() {
            return;
        }

        let mut caches = self.caches.lock();
        caches.entry(vm.hash).or_default().push(vm);

        let total = caches.values().map(|v| v.len()).sum::<usize>();
        if total > 64 {
            let mut rng = rand::thread_rng();
            let rm_idx = rng.gen_range(0..total);

            let mut idx = 0;
            let mut rm_key = None;
            for (k, v) in caches.iter_mut() {
                if idx + v.len() > rm_idx {
                    v.pop();
                    if v.is_empty() {
                        rm_key = Some(*k);
                    }
                    break;
                }
                idx += v.len();
            }
            if let Some(k) = rm_key {
                caches.remove(&k);
            }
        }
    }
}
