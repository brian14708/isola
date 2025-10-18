use std::{
    path::{Path, PathBuf},
    pin::Pin,
};

use bytes::Bytes;
use futures::Stream;
use tokio::io::AsyncWriteExt;
use zip::ZipArchive;

use crate::{
    env::EnvHandle,
    error::Error,
    vm::{
        Vm, VmIterator,
        exports::{Argument as VmArgument, Value},
    },
};

enum ExecArgumentValue {
    Cbor(Bytes),
    CborStream(Pin<Box<dyn Stream<Item = Bytes> + Send>>),
}

pub struct Argument {
    name: Option<String>,
    value: ExecArgumentValue,
}

pub enum ArgumentOwned {
    Cbor(Option<String>, Bytes),
    CborIterator(Option<String>, Option<VmIterator>),
}

impl ArgumentOwned {
    pub fn as_value(&mut self) -> VmArgument<'_> {
        match self {
            ArgumentOwned::Cbor(name, value) => VmArgument {
                name: name.as_deref(),
                value: Value::Cbor(AsRef::<[u8]>::as_ref(value)),
            },
            ArgumentOwned::CborIterator(name, value) => VmArgument {
                name: name.as_deref(),
                value: Value::CborIterator(
                    value.take().expect("CborIterator must be only used once"),
                ),
            },
        }
    }
}

impl Argument {
    #[must_use]
    pub fn cbor(name: Option<String>, value: impl Into<Bytes>) -> Self {
        Self {
            name,
            value: ExecArgumentValue::Cbor(value.into()),
        }
    }

    #[must_use]
    pub fn cbor_stream(
        name: Option<String>,
        value: impl Stream<Item = Bytes> + Send + 'static,
    ) -> Self {
        Self {
            name,
            value: ExecArgumentValue::CborStream(Box::pin(value)),
        }
    }

    pub(crate) fn into_owned<E>(self, vm: &mut Vm<E>) -> anyhow::Result<ArgumentOwned>
    where
        E: EnvHandle,
    {
        match self.value {
            ExecArgumentValue::CborStream(v) => Ok(ArgumentOwned::CborIterator(
                self.name,
                Some(vm.new_iter(v)?),
            )),
            ExecArgumentValue::Cbor(v) => Ok(ArgumentOwned::Cbor(self.name, v)),
        }
    }
}

pub enum StreamItem {
    Data(Bytes),
    End(Option<Bytes>),
    Error(Error),
}

pub enum Source {
    Script { prelude: String, code: String },
    Bundle(Bytes),
    Path(PathBuf),
}

impl Source {
    pub(crate) async fn extract_zip(data: impl AsRef<[u8]>, dest: &Path) -> anyhow::Result<()> {
        let data = data.as_ref();
        let cursor = std::io::Cursor::new(data);
        let mut archive = ZipArchive::new(cursor)?;
        let mut dirs = std::collections::HashSet::new();
        let mut buffer = vec![0u8; 16384]; // 16KB buffer reused across files

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let outpath = match file.enclosed_name() {
                Some(path) => dest.join(path),
                None => continue,
            };

            if file.name().ends_with('/') {
                // Directory
                if !dirs.contains(&outpath) {
                    tokio::fs::create_dir_all(&outpath).await?;
                    dirs.insert(outpath);
                }
            } else {
                // File
                if let Some(p) = outpath.parent()
                    && !dirs.contains(p)
                {
                    tokio::fs::create_dir_all(&p).await?;
                    dirs.insert(p.to_owned());
                }

                // Stream file contents in chunks
                let mut out_file = tokio::fs::File::create(&outpath).await?;
                loop {
                    let bytes_read = std::io::Read::read(&mut file, &mut buffer)?;
                    if bytes_read == 0 {
                        break;
                    }
                    out_file.write_all(&buffer[..bytes_read]).await?;
                }
            }
        }
        Ok(())
    }
}
