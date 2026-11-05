// use tracing;

use bytesize::ByteSize;

use ltrace::{RollingWriter, Rotation};
use tracing_appender::non_blocking;
use tracing_appender::non_blocking::{WorkerGuard};
use tracing_subscriber::{
	layer::SubscriberExt,
	util::SubscriberInitExt,
	EnvFilter,
};
use tracing_subscriber::fmt::{
	format::Writer,
	time::{FormatTime, SystemTime},
};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::util::TryInitError;

use std::io::IsTerminal;
use std::{
	collections::HashSet,
	error::Error,
	ffi::OsStr,
	fs,
	io,
	path::{Path, PathBuf},
};

use nmrs::{ActiveConnection, NetworkManager};



#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum StdoutTimestamps {
	/// Automatically detect based on whether stdout is a terminal
	Auto,

	Always,
	Never,
}


struct ConditionalTimer {
	enabled: bool,
}

impl FormatTime for ConditionalTimer {
	fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
		if self.enabled {
			SystemTime.format_time(w)
		} else {
			Ok(())
		}
	}
}



#[derive(Debug, thiserror::Error)]
pub enum InitLoggingError {
	#[error("failed to build rolling log writer for {path}")]
	RollingWriter {
		path: String,
		#[source]
		source: std::io::Error,
	},

	#[error(
		"failed to initialize tracing subscriber\npath: {}\nfilter: {filter}",
		path.as_deref().unwrap_or("<stdout only>")
	)]
	SubscriberInit {
		path: Option<String>,
		filter: String,
		#[source]
		source: TryInitError,
	},
}

pub fn init_logging(
	path: Option<impl AsRef<Path> + std::fmt::Display>,
	level: LevelFilter,
	stdout_timestamps: StdoutTimestamps,
	max_file_size: ByteSize,
	max_files: usize,
) -> Result<Option<WorkerGuard>, InitLoggingError> {

	let filter = EnvFilter::try_from_default_env()
		.unwrap_or_else(|_| EnvFilter::new(level.to_string()));

	let enable_stdout_timestamps = match stdout_timestamps {
		StdoutTimestamps::Auto => std::io::stdout().is_terminal(),
		StdoutTimestamps::Always => true,
		StdoutTimestamps::Never => false,
	};

	let stdout_layer = tracing_subscriber::fmt::layer()
		.with_writer(std::io::stdout)
		.with_timer(ConditionalTimer {
			enabled: enable_stdout_timestamps,
		});

	if let Some(path) = path {
		let writer = RollingWriter::builder(&path)
			.rotation(Rotation::Never)
			.max_file_size(max_file_size.as_u64())
			.max_files(max_files)
			.build()
			.map_err(|source| InitLoggingError::RollingWriter {
				path: path.to_string(),
				source,
			})?;

		let (file_writer, guard) = non_blocking(writer);

		tracing_subscriber::registry()
			.with(filter.clone())
			.with(stdout_layer)
			.with(
				tracing_subscriber::fmt::layer()
					.with_writer(file_writer)
					.with_ansi(false),
			)
			.try_init()
			.map_err(|source| InitLoggingError::SubscriberInit {
				path: Some(path.to_string()),
				filter: filter.to_string(),
				source,
			})?;

		Ok(Some(guard))
	} else {
		tracing_subscriber::registry()
			.with(filter.clone())
			.with(stdout_layer)
			.try_init()
			.map_err(|source| InitLoggingError::SubscriberInit {
				path: None,
				filter: filter.to_string(),
				source,
			})?;

		Ok(None)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nice(i8);

impl std::fmt::Display for Nice {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		self.0.fmt(f)
	}
}

#[derive(Debug, thiserror::Error)]
pub enum SetNiceError {
	#[error("failed to set niceness: {0}")]
	Set(#[source] io::Error),

	#[error("failed to retrieve niceness: {0}")]
	Get(#[source] io::Error),

	#[error("niceness mismatch: requested {requested}, got {actual}")]
	Mismatch {
		requested: Nice,
		actual: Nice,
	},
}

macro_rules! impl_try_from_int {
	($($ty:ty),*) => {
		$(
			impl TryFrom<$ty> for Nice {
				type Error = std::io::Error;

				fn try_from(value: $ty) -> Result<Self, Self::Error> {
					let value = i8::try_from(value).map_err(|_| {
						std::io::Error::from(
							std::io::ErrorKind::InvalidInput
						)
					})?;

					if (-20..=19).contains(&value) {
						Ok(Self(value))
					} else {
						Err(std::io::Error::from(
							std::io::ErrorKind::InvalidInput
						))
					}
				}
			}
		)*
	};
}

impl_try_from_int!(
	i8, i16, i32, i64, i128, isize,
	u8, u16, u32, u64, u128, usize
);

macro_rules! impl_from_nice {
	($($ty:ty),*) => {
		$(
			impl From<Nice> for $ty {
				fn from(nice: Nice) -> Self {
					nice.0 as $ty
				}
			}
		)*
	};
}

impl_from_nice!(
	i8, i16, i32, i64, i128, isize,
	u8, u16, u32, u64, u128, usize
);

/// Retrieves the calling thread's niceness.
pub fn get_current_niceness() -> io::Result<Nice> {
	unsafe {
		// getpriority() can legitimately return -1.
		*libc::__errno_location() = 0;

		let priority = libc::getpriority(libc::PRIO_PROCESS, 0);
		let errno = *libc::__errno_location();

		if priority == -1 && errno != 0 {
			return Err(io::Error::from_raw_os_error(errno));
		}

		Nice::try_from(priority)
	}
}

/// Sets and verifies the calling thread's niceness.
pub fn set_niceness(nice: Nice) -> Result<(), SetNiceError> {
	let result = unsafe {
		libc::setpriority(
			libc::PRIO_PROCESS,
			0,
			nice.into(),
		)
	};

	if result == -1 {
		return Err(SetNiceError::Set(
			io::Error::last_os_error(),
		));
	}

	let actual = get_current_niceness()
		.map_err(SetNiceError::Get)?;

	if actual != nice {
		return Err(SetNiceError::Mismatch {
			requested: nice,
			actual,
		});
	}

	Ok(())
}


#[derive(Debug, thiserror::Error)]
pub enum CleanupConnectionFilesError {
	#[error(transparent)]
	NetworkManager(#[from] nmrs::ConnectionError),

	#[error(transparent)]
	Io(#[from] io::Error),

	#[error("File extension cannot be empty")]
	EmptyExtension,

	#[error("Unknown NetworkManager connection type; refusing to clean up")]
	UnknownConnectionType,
}

pub async fn cleanup_connection_files(
	dir: impl AsRef<Path>,
	extension: &str,
) -> Result<Vec<PathBuf>, CleanupConnectionFilesError> {
	let extension = extension.strip_prefix('.').unwrap_or(extension);

	if extension.is_empty() {
		return Err(CleanupConnectionFilesError::EmptyExtension);
	}

	let nm = NetworkManager::new().await?;
	let connections = nm.list_active_connections().await?;

	let mut active_uuids: HashSet<String> = HashSet::with_capacity(connections.len());

	for connection in connections {
		let uuid = match connection {
			ActiveConnection::Wired(c) => c.uuid,
			ActiveConnection::Wifi(c) => c.uuid,
			ActiveConnection::Vpn(c) => c.uuid,
			ActiveConnection::Other(c) => c.uuid,
			_ => return Err(CleanupConnectionFilesError::UnknownConnectionType),
		};

		active_uuids.insert(uuid);
	}

	let mut removed_files = Vec::new();

	for entry in fs::read_dir(dir)? {
		let entry = entry?;
		let file_type = entry.file_type()?;

		if !file_type.is_file() {
			continue;
		}

		let path = entry.path();

		if path.extension() != Some(OsStr::new(extension)) {
			continue;
		}

		let is_active = path
			.file_stem()
			.and_then(OsStr::to_str)
			.is_some_and(|uuid| active_uuids.contains(uuid));

		if !is_active {
			fs::remove_file(&path)?;
			removed_files.push(path);
		}
	}

	Ok(removed_files)
}



#[cfg(test)]
mod tests {
	use super::*;

	fn test_set_niceness(n: i32) {
		let nice = Nice::try_from(n).unwrap();
		set_niceness(nice).unwrap();
		println!("{:#?}", get_current_niceness());
	}

	#[test]
	fn test_many_set_nicenesses() {
		//WARN: Should be in order or else you'll get perm errors raising niceness
		test_set_niceness(5);
		test_set_niceness(15);
		test_set_niceness(19);
	}

	#[test]
	fn test_nice_type() {
		assert!(Nice::try_from(-21).is_err());
		assert!(Nice::try_from(20).is_err());

		assert!(Nice::try_from(-20).is_ok());
		assert!(Nice::try_from(19).is_ok());
	}
}
