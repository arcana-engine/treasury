use std::{
    collections::{hash_map::Entry, HashMap},
    path::{Path, PathBuf},
    // time::SystemTime,
};

use rand::random;

struct Temporary {
    path: PathBuf,
    // last_access: SystemTime,
}

/// Container for temporary files.
pub struct Temporaries<'a> {
    base: &'a Path,
    map: HashMap<u128, Temporary>,
}

impl<'a> Temporaries<'a> {
    pub fn new(base: &'a Path) -> Self {
        Temporaries {
            base,
            map: HashMap::new(),
        }
    }

    pub fn make_temporary(&mut self) -> PathBuf {
        let tmp = loop {
            let key = random();
            match self.map.entry(key) {
                Entry::Occupied(_) => continue,
                Entry::Vacant(entry) => {
                    break entry.insert(Temporary {
                        path: {
                            let key_bytes = key.to_le_bytes();
                            let mut filename = [0; 22];
                            let len = base64::encode_config_slice(
                                &key_bytes,
                                base64::URL_SAFE_NO_PAD,
                                &mut filename,
                            );
                            debug_assert_eq!(len, 22);
                            self.base.join(std::str::from_utf8(&filename).unwrap())
                        },
                        // last_access: SystemTime::now(),
                    });
                }
            }
        };
        tmp.path.clone()
    }
}
