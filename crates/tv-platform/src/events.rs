//! Windows event logs, read with Windows' own event log API: saved `.evtx`
//! files, and this PC's logs by name, such as `System`. Messages are written
//! out as Event Viewer writes them, from the message files of the providers
//! installed on this PC, which Windows reads as data.

use std::collections::HashMap;
use std::path::Path;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS};
use windows::Win32::System::EventLog::{
    EvtClose, EvtCreateRenderContext, EvtFormatMessage, EvtFormatMessageEvent, EvtGetLogInfo,
    EvtLogNumberOfLogRecords, EvtNext, EvtNextChannelPath, EvtOpenChannelEnum, EvtOpenChannelPath,
    EvtOpenFilePath, EvtOpenLog, EvtOpenPublisherMetadata, EvtQuery, EvtQueryChannelPath,
    EvtQueryFilePath, EvtQueryForwardDirection, EvtRender, EvtRenderContextSystem,
    EvtRenderContextUser, EvtRenderEventValues, EvtSeek, EvtSeekRelativeToFirst, EvtSystemComputer,
    EvtSystemEventID, EvtSystemEventRecordId, EvtSystemLevel, EvtSystemProviderName,
    EvtSystemTimeCreated, EvtVarTypeAnsiString, EvtVarTypeBinary, EvtVarTypeBoolean,
    EvtVarTypeByte, EvtVarTypeDouble, EvtVarTypeFileTime, EvtVarTypeGuid, EvtVarTypeHexInt32,
    EvtVarTypeHexInt64, EvtVarTypeInt16, EvtVarTypeInt32, EvtVarTypeInt64, EvtVarTypeSByte,
    EvtVarTypeSingle, EvtVarTypeString, EvtVarTypeUInt16, EvtVarTypeUInt32, EvtVarTypeUInt64,
    EVT_HANDLE, EVT_VARIANT, EVT_VARIANT_TYPE_ARRAY,
};

/// Where events come from.
#[derive(Clone, Copy, Debug)]
pub enum EventSource<'a> {
    /// A saved `.evtx` file, here or on another PC's share.
    File(&'a Path),
    /// One of this PC's logs, by the name Event Viewer shows under
    /// Applications and Services Logs, such as `System` or
    /// `Microsoft-Windows-TaskScheduler/Operational`.
    Log(&'a str),
}

/// One event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    /// Its number in its log, which only goes up.
    pub record: u64,
    /// When it happened, in 100-nanosecond steps since 1601, in UTC.
    pub time: u64,
    /// 1 is critical, 2 error, 3 warning, 4 information and 5 verbose. 0 is
    /// an event logged whatever the level, which reads as information.
    pub level: u8,
    pub provider: String,
    pub id: u16,
    pub computer: String,
    /// The message, as Event Viewer shows it, or the event's own values when
    /// this PC has no message for it.
    pub message: String,
}

/// An event log handle, closed when dropped.
struct Handle(EVT_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from the event log API and is closed only here.
        unsafe {
            let _ = EvtClose(self.0);
        }
    }
}

fn is(error: &windows::core::Error, code: windows::Win32::Foundation::WIN32_ERROR) -> bool {
    error.code() == code.to_hresult()
}

fn text(error: &windows::core::Error) -> String {
    let message = error.message();
    let message = message.trim().trim_end_matches('.');
    if message.is_empty() {
        format!("Windows error 0x{:08X}", error.code().0)
    } else {
        message.to_string()
    }
}

/// Events read by each thread at a time when the log's size isn't known.
/// Messages take most of the time, so stretches of the log are read on every
/// core at once.
const STRETCH: usize = 4096;
/// The fewest events a thread is given, as each starts its own query.
const LEAST_STRETCH: usize = 1024;

/// Reads a source's events, oldest first, that come after record `after`,
/// or all of them for 0. `each` is called with each event, in order, and
/// reading stops early if it returns false. Returns how many were read.
///
/// Writing out each message takes most of the time, so the log is read in
/// stretches on every core at once, each thread seeking to its own, and the
/// stretches are handed to `each` in order.
pub fn read_events(
    source: EventSource<'_>,
    after: u64,
    mut each: impl FnMut(Event) -> bool,
) -> Result<usize, String> {
    let threads = std::thread::available_parallelism().map_or(4, |count| count.get().min(16));
    // A whole log is split evenly, so every thread finishes at about the
    // same time. Events added since the count are read in later rounds.
    let stretch = match (after, log_size(source)) {
        (0, Some(size)) => size.div_ceil(threads).max(LEAST_STRETCH),
        _ => STRETCH,
    };
    let mut next = 0;
    let mut count = 0;
    loop {
        let stretches: Vec<Result<Vec<Event>, String>> = std::thread::scope(|scope| {
            let started: Vec<_> = (0..threads)
                .map(|index| {
                    let first = (next + index) * stretch;
                    scope.spawn(move || read_stretch(source, after, first, stretch))
                })
                .collect();
            started
                .into_iter()
                .map(|thread| {
                    thread
                        .join()
                        .unwrap_or_else(|_| Err("reading the events failed".to_string()))
                })
                .collect()
        });
        next += threads;
        for read in stretches {
            let events = read?;
            let whole = events.len() == stretch;
            for event in events {
                count += 1;
                if !each(event) {
                    return Ok(count);
                }
            }
            if !whole {
                return Ok(count);
            }
        }
    }
}

/// How many events a log holds, if Windows says.
fn log_size(source: EventSource<'_>) -> Option<usize> {
    let (path, flags) = match source {
        EventSource::File(path) => (HSTRING::from(path.as_os_str()), EvtOpenFilePath.0),
        EventSource::Log(name) => (HSTRING::from(name), EvtOpenChannelPath.0),
    };
    // SAFETY: the path outlives the call, the log handle is closed by
    // `Handle`, and the value is read into a buffer of its own size.
    unsafe {
        let log = Handle(EvtOpenLog(None, &path, flags).ok()?);
        let mut value = EVT_VARIANT::default();
        let mut used = 0u32;
        EvtGetLogInfo(
            log.0,
            EvtLogNumberOfLogRecords,
            std::mem::size_of::<EVT_VARIANT>() as u32,
            Some(&mut value),
            &mut used,
        )
        .ok()?;
        (value.Type == EvtVarTypeUInt64.0 as u32)
            .then(|| usize::try_from(value.Anonymous.UInt64Val).ok())
            .flatten()
    }
}

/// Up to `most` events from the `first`th on, of those after record `after`.
fn read_stretch(
    source: EventSource<'_>,
    after: u64,
    first: usize,
    most: usize,
) -> Result<Vec<Event>, String> {
    let (path, flags) = match source {
        EventSource::File(path) => (HSTRING::from(path.as_os_str()), EvtQueryFilePath.0),
        EventSource::Log(name) => (HSTRING::from(name), EvtQueryChannelPath.0),
    };
    let query = if after == 0 {
        "*".to_string()
    } else {
        format!("*[System[EventRecordID>{after}]]")
    };
    // SAFETY: the strings outlive the calls, every handle is closed by
    // `Handle`, and each event handle is used only while it's open.
    unsafe {
        let results = Handle(
            EvtQuery(
                None,
                &path,
                &HSTRING::from(query),
                flags | EvtQueryForwardDirection.0,
            )
            .map_err(|error| text(&error))?,
        );
        // A stretch past the last event is empty.
        if first > 0
            && EvtSeek(
                results.0,
                first as i64,
                None,
                None,
                EvtSeekRelativeToFirst.0,
            )
            .is_err()
        {
            return Ok(Vec::new());
        }
        let system =
            Handle(EvtCreateRenderContext(None, EvtRenderContextSystem.0).map_err(|e| text(&e))?);
        let user =
            Handle(EvtCreateRenderContext(None, EvtRenderContextUser.0).map_err(|e| text(&e))?);
        let mut reader = Reader {
            system,
            user,
            publishers: HashMap::new(),
            buffer: Vec::new(),
        };
        let mut events = Vec::with_capacity(most);
        while events.len() < most {
            let mut handles = [0isize; 128];
            let wanted = (most - events.len()).min(handles.len());
            let mut returned = 0u32;
            match EvtNext(
                results.0,
                &mut handles[..wanted],
                u32::MAX,
                0,
                &mut returned,
            ) {
                Ok(()) => {}
                Err(error) if is(&error, ERROR_NO_MORE_ITEMS) => break,
                Err(error) => return Err(text(&error)),
            }
            // Every handle returned is wrapped first, so all are closed.
            let handles: Vec<Handle> = handles[..returned as usize]
                .iter()
                .map(|handle| Handle(EVT_HANDLE(*handle)))
                .collect();
            for event in &handles {
                events.push(reader.read(event));
            }
        }
        Ok(events)
    }
}

/// What reading events needs from one event to the next.
struct Reader {
    system: Handle,
    user: Handle,
    /// Each provider's message file, opened once, or none if this PC hasn't
    /// got it.
    publishers: HashMap<String, Option<Handle>>,
    /// Holds rendered values, as u64s so they're aligned as the values need.
    buffer: Vec<u64>,
}

impl Reader {
    /// # Safety
    ///
    /// `event` must be an open event handle.
    unsafe fn read(&mut self, event: &Handle) -> Event {
        // SAFETY: the caller's handle is open, and the values rendered are
        // read before the buffer is used again.
        unsafe {
            let mut read = Event::default();
            if let Some(values) = render(&self.system, event, &mut self.buffer) {
                let value = |id: i32| values.get(id as usize);
                read.provider = value(EvtSystemProviderName.0)
                    .and_then(|value| string(value))
                    .unwrap_or_default();
                read.id = value(EvtSystemEventID.0)
                    .filter(|value| value.Type == EvtVarTypeUInt16.0 as u32)
                    .map_or(0, |value| value.Anonymous.UInt16Val);
                read.level = value(EvtSystemLevel.0)
                    .filter(|value| value.Type == EvtVarTypeByte.0 as u32)
                    .map_or(0, |value| value.Anonymous.ByteVal);
                read.time = value(EvtSystemTimeCreated.0)
                    .filter(|value| value.Type == EvtVarTypeFileTime.0 as u32)
                    .map_or(0, |value| value.Anonymous.FileTimeVal);
                read.record = value(EvtSystemEventRecordId.0)
                    .filter(|value| value.Type == EvtVarTypeUInt64.0 as u32)
                    .map_or(0, |value| value.Anonymous.UInt64Val);
                read.computer = value(EvtSystemComputer.0)
                    .and_then(|value| string(value))
                    .unwrap_or_default();
            }
            let publisher = self
                .publishers
                .entry(read.provider.clone())
                .or_insert_with(|| open_publisher(&read.provider));
            read.message = match publisher
                .as_ref()
                .and_then(|publisher| message(publisher, event))
            {
                Some(message) => message,
                None => {
                    let values = render(&self.user, event, &mut self.buffer)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(|value| value_text(value))
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    if values.is_empty() {
                        "This PC has no text for this event.".to_string()
                    } else {
                        format!("This PC has no text for this event. Its values: {values}")
                    }
                }
            };
            read
        }
    }
}

/// Renders an event's values with a render context, into `buffer`.
///
/// # Safety
///
/// `event` must be an open event handle, and the values returned point into
/// `buffer` and the event, so they're good only until either changes.
unsafe fn render<'a>(
    context: &Handle,
    event: &Handle,
    buffer: &'a mut Vec<u64>,
) -> Option<&'a [EVT_VARIANT]> {
    // SAFETY: the buffer's size is given in bytes, and a second call has the
    // size the first asked for.
    unsafe {
        for _ in 0..2 {
            let (mut used, mut count) = (0u32, 0u32);
            let bytes = (buffer.len() * 8) as u32;
            let pointer = (!buffer.is_empty()).then(|| buffer.as_mut_ptr().cast());
            match EvtRender(
                Some(context.0),
                event.0,
                EvtRenderEventValues.0,
                bytes,
                pointer,
                &mut used,
                &mut count,
            ) {
                Ok(()) => {
                    let values = buffer.as_ptr().cast::<EVT_VARIANT>();
                    return Some(std::slice::from_raw_parts(values, count as usize));
                }
                Err(error) if is(&error, ERROR_INSUFFICIENT_BUFFER) => {
                    buffer.resize((used as usize).div_ceil(8), 0);
                }
                Err(_) => return None,
            }
        }
        None
    }
}

/// A provider's message file, if this PC has it.
fn open_publisher(provider: &str) -> Option<Handle> {
    if provider.is_empty() {
        return None;
    }
    // SAFETY: the name outlives the call.
    unsafe {
        EvtOpenPublisherMetadata(None, &HSTRING::from(provider), PCWSTR::null(), 0, 0)
            .ok()
            .map(Handle)
    }
}

/// An event's message, as Event Viewer shows it.
///
/// # Safety
///
/// Both handles must be open.
unsafe fn message(publisher: &Handle, event: &Handle) -> Option<String> {
    // SAFETY: the first call only measures, and the second's buffer has that
    // size. A message missing some values still comes back, as Event Viewer
    // shows it.
    unsafe {
        let mut used = 0u32;
        let _ = EvtFormatMessage(
            Some(publisher.0),
            Some(event.0),
            0,
            None,
            EvtFormatMessageEvent.0,
            None,
            &mut used,
        );
        if used == 0 {
            return None;
        }
        let mut buffer = vec![0u16; used as usize];
        let result = EvtFormatMessage(
            Some(publisher.0),
            Some(event.0),
            0,
            None,
            EvtFormatMessageEvent.0,
            Some(&mut buffer),
            &mut used,
        );
        // Values that couldn't be put in leave their places marked.
        let partial = |error: &windows::core::Error| {
            matches!((error.code().0 as u32) & 0xFFFF, 15029..=15032)
        };
        if result.as_ref().is_err_and(|error| !partial(error)) {
            return None;
        }
        let end = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        let message = String::from_utf16_lossy(&buffer[..end]);
        let message = message.trim_end();
        (!message.is_empty()).then(|| message.to_string())
    }
}

/// A string value's text.
///
/// # Safety
///
/// The value must have been rendered into a buffer that's still unchanged.
unsafe fn string(value: &EVT_VARIANT) -> Option<String> {
    // SAFETY: a string value points at a nul-terminated string in the
    // rendered buffer, checked for null first.
    unsafe {
        (value.Type == EvtVarTypeString.0 as u32 && !value.Anonymous.StringVal.is_null())
            .then(|| String::from_utf16_lossy(value.Anonymous.StringVal.as_wide()))
    }
}

/// An event's own value as text, for an event this PC has no message for.
/// Lists of values are left out.
///
/// # Safety
///
/// The value must have been rendered into a buffer that's still unchanged.
unsafe fn value_text(value: &EVT_VARIANT) -> Option<String> {
    if value.Type & EVT_VARIANT_TYPE_ARRAY != 0 {
        return None;
    }
    let kind = value.Type as i32;
    // SAFETY: each arm reads the union as the type the value says it is, and
    // pointers are checked for null first.
    unsafe {
        let data = &value.Anonymous;
        Some(match kind {
            k if k == EvtVarTypeString.0 => return string(value),
            k if k == EvtVarTypeAnsiString.0 => {
                if data.AnsiStringVal.is_null() {
                    return None;
                }
                String::from_utf8_lossy(data.AnsiStringVal.as_bytes()).into_owned()
            }
            k if k == EvtVarTypeSByte.0 => data.SByteVal.to_string(),
            k if k == EvtVarTypeByte.0 => data.ByteVal.to_string(),
            k if k == EvtVarTypeInt16.0 => data.Int16Val.to_string(),
            k if k == EvtVarTypeUInt16.0 => data.UInt16Val.to_string(),
            k if k == EvtVarTypeInt32.0 => data.Int32Val.to_string(),
            k if k == EvtVarTypeUInt32.0 => data.UInt32Val.to_string(),
            k if k == EvtVarTypeInt64.0 => data.Int64Val.to_string(),
            k if k == EvtVarTypeUInt64.0 => data.UInt64Val.to_string(),
            k if k == EvtVarTypeHexInt32.0 => format!("0x{:X}", data.UInt32Val),
            k if k == EvtVarTypeHexInt64.0 => format!("0x{:X}", data.UInt64Val),
            k if k == EvtVarTypeSingle.0 => data.SingleVal.to_string(),
            k if k == EvtVarTypeDouble.0 => data.DoubleVal.to_string(),
            k if k == EvtVarTypeBoolean.0 => data.BooleanVal.as_bool().to_string(),
            k if k == EvtVarTypeGuid.0 => {
                let guid = data.GuidVal.as_ref()?;
                format!("{{{guid:?}}}")
            }
            k if k == EvtVarTypeBinary.0 => {
                if data.BinaryVal.is_null() {
                    return None;
                }
                let bytes =
                    std::slice::from_raw_parts(data.BinaryVal, (value.Count as usize).min(64));
                bytes.iter().map(|byte| format!("{byte:02X}")).collect()
            }
            _ => return None,
        })
    }
}

/// The names of this PC's event logs, as Event Viewer lists them.
pub fn log_names() -> Result<Vec<String>, String> {
    // SAFETY: the enumeration handle is closed by `Handle`, and each call's
    // buffer is given with its length, grown when Windows asks for more.
    unsafe {
        let names = Handle(EvtOpenChannelEnum(None, 0).map_err(|error| text(&error))?);
        let mut found = Vec::new();
        let mut buffer = vec![0u16; 512];
        loop {
            let mut used = 0u32;
            match EvtNextChannelPath(names.0, Some(&mut buffer), &mut used) {
                Ok(()) => {
                    let end = buffer
                        .iter()
                        .position(|unit| *unit == 0)
                        .unwrap_or(buffer.len());
                    found.push(String::from_utf16_lossy(&buffer[..end]));
                }
                Err(error) if is(&error, ERROR_INSUFFICIENT_BUFFER) => {
                    buffer.resize(used as usize, 0);
                }
                Err(error) if is(&error, ERROR_NO_MORE_ITEMS) => return Ok(found),
                Err(error) => return Err(text(&error)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_pcs_logs_are_listed() {
        let names = log_names().unwrap();
        assert!(
            names.iter().any(|name| name == "System"),
            "{} logs",
            names.len()
        );
        assert!(names.iter().any(|name| name == "Application"));
    }

    #[test]
    fn the_system_log_reads_with_messages() {
        let mut events = Vec::new();
        read_events(EventSource::Log("System"), 0, |event| {
            events.push(event);
            events.len() < 200
        })
        .unwrap();
        assert!(!events.is_empty(), "the System log has events");
        // Oldest first, and each with what it needs.
        assert!(events
            .windows(2)
            .all(|pair| pair[0].record < pair[1].record));
        assert!(events
            .iter()
            .all(|event| event.time > 0 && !event.provider.is_empty()));
        assert!(events.iter().all(|event| !event.message.is_empty()));
        let with_text = events
            .iter()
            .filter(|event| !event.message.starts_with("This PC has no text"))
            .count();
        assert!(
            with_text * 2 > events.len(),
            "most have their text: {with_text} of {}",
            events.len()
        );

        // Reading after a record gives only the newer events.
        let last = events.last().unwrap().record;
        let mut newer = Vec::new();
        read_events(EventSource::Log("System"), last, |event| {
            newer.push(event.record);
            newer.len() < 10
        })
        .unwrap();
        assert!(newer.iter().all(|record| *record > last));
    }

    /// Stretches are read on several threads at once, and must still give
    /// every event once, in order.
    #[test]
    fn a_whole_log_reads_each_event_once_in_order() {
        let size = log_size(EventSource::Log("System")).expect("Windows says the size");
        let mut records = Vec::new();
        read_events(EventSource::Log("System"), 0, |event| {
            records.push(event.record);
            true
        })
        .unwrap();
        assert!(
            records.windows(2).all(|pair| pair[0] < pair[1]),
            "in order, once each"
        );
        // Events may arrive while it's read, but none are missed.
        assert!(records.len() >= size, "{} read of {size}", records.len());
    }

    #[test]
    fn a_missing_log_or_file_says_why() {
        assert!(read_events(EventSource::Log("No-Such-Log/Anywhere"), 0, |_| true).is_err());
        let dir = std::env::temp_dir().join("traceview-no-such-folder");
        assert!(read_events(EventSource::File(&dir.join("missing.evtx")), 0, |_| true).is_err());
    }
}
