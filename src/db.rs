//! Single-database, memory-only string commands, serialized by the caller.
//!
//! Memory admission uses an explicit conservative accounting model, not RSS:
//! `ENTRY_OVERHEAD + key.len() + value.len()` plus `EXPIRY_OVERHEAD` for a TTL.
//! Fixed charges allow for handles, allocation metadata, spare hash buckets and
//! B-tree nodes on supported native targets. The ordered index shares key bytes
//! with the map and contains exactly one record per expiring key, never tombstones.
//! Retained map capacity, command/response buffers and transient replacement
//! copies are not live-entry accounting; embedders must budget these separately.
//!
//! All semantic/admission checks precede writes. Reclaiming already-expired keys
//! is permitted even when a write fails. OOM admission reclaims at most
//! `OOM_EXPIRE_BUDGET` unrelated expired keys, so a retry after background expiry
//! can succeed. There is no eviction, persistence, authentication or connection
//! state here. Callers must supply a nondecreasing monotonic `now`.

use crate::protocol::Frame;
use bytes::Bytes;
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

/// Conservative fixed accounting charge for one stored key/value pair.
pub const ENTRY_OVERHEAD: usize = 512;
/// Additional fixed accounting charge for one removable expiration-index record.
pub const EXPIRY_OVERHEAD: usize = 1024;
/// Maximum unrelated expired keys reclaimed by a single write-admission attempt.
pub const OOM_EXPIRE_BUDGET: usize = 64;

/// Exact live-entry accounting formula; `None` means arithmetic overflow.
pub fn entry_memory_cost(key_bytes: usize, value_bytes: usize, expiring: bool) -> Option<usize> {
    ENTRY_OVERHEAD
        .checked_add(key_bytes)?
        .checked_add(value_bytes)?
        .checked_add(if expiring { EXPIRY_OVERHEAD } else { 0 })
}

struct Entry {
    value: Bytes,
    deadline: Option<Instant>,
    cost: usize,
}

/// A string keyspace with checked write admission and an ordered expiry index.
pub struct Db {
    // RandomState is intentional: clients choose arbitrary binary keys.
    entries: HashMap<Bytes, Entry>,
    expirations: BTreeSet<(Instant, Bytes)>,
    used: usize,
    max_memory: usize,
}

impl Db {
    /// Construct an empty database. Zero is a zero-byte budget, not unlimited.
    pub fn new(max_memory: usize) -> Self {
        Self {
            entries: HashMap::new(),
            expirations: BTreeSet::new(),
            used: 0,
            max_memory,
        }
    }

    /// Accounted live allocations, including expired keys not yet reclaimed.
    pub fn memory_used(&self) -> usize {
        self.used
    }

    /// Remove at most `budget` due keys, ordered by deadline, without scanning.
    pub fn expire(&mut self, now: Instant, budget: usize) {
        for _ in 0..budget {
            let Some((deadline, key)) = self.expirations.first() else {
                break;
            };
            if *deadline > now {
                break;
            }
            let key = key.clone();
            self.take(&key);
        }
    }

    /// Execute one command atomically with respect to other calls on this object.
    /// Connection commands are advertised but must be handled by the server.
    pub fn execute(&mut self, args: &[Bytes], now: Instant) -> Frame {
        match self.run(args, now) {
            Ok(frame) => frame,
            Err(error) => error.frame(),
        }
    }

    fn run(&mut self, args: &[Bytes], now: Instant) -> CommandResult {
        let Some(name) = args.first() else {
            return Err(CommandError::Syntax);
        };
        let command = COMMANDS
            .iter()
            .find(|c| name.eq_ignore_ascii_case(c.name.as_bytes()))
            .ok_or(CommandError::Unknown)?;
        if !command.accepts(args.len()) {
            return Err(CommandError::Arity(command.name));
        }
        // All positional indexing below is guarded by this table's arity check.
        match command.name {
            "get" => {
                self.touch(&args[1], now);
                Ok(self.value_frame(&args[1]))
            }
            "set" => self.set(args, now),
            "mget" => {
                let mut values = Vec::new();
                values
                    .try_reserve_exact(args.len() - 1)
                    .map_err(|_| CommandError::Oom)?;
                for key in &args[1..] {
                    self.touch(key, now);
                    values.push(self.value_frame(key));
                }
                Ok(Frame::Array(values))
            }
            "mset" => self.mset(args, now),
            "del" | "exists" => {
                let mut count = 0;
                for key in &args[1..] {
                    self.touch(key, now);
                    // EXISTS counts duplicate live keys; DEL counts actual removals.
                    if if command.name == "del" {
                        self.take(key).is_some()
                    } else {
                        self.entries.contains_key(key.as_ref())
                    } {
                        count += 1;
                    }
                }
                Ok(Frame::Integer(count))
            }
            "type" => {
                self.touch(&args[1], now);
                Ok(Frame::Simple(
                    if self.entries.contains_key(args[1].as_ref()) {
                        "string"
                    } else {
                        "none"
                    }
                    .into(),
                ))
            }
            "append" => self.append(args, now),
            "strlen" => {
                self.touch(&args[1], now);
                Ok(integer_size(
                    self.entries
                        .get(args[1].as_ref())
                        .map_or(0, |e| e.value.len()),
                ))
            }
            "incr" | "incrby" | "decr" | "decrby" => self.counter(args, command.name, now),
            "expire" | "pexpire" => self.expire_command(args, command.name == "expire", now),
            "ttl" | "pttl" => {
                self.touch(&args[1], now);
                let Some(entry) = self.entries.get(args[1].as_ref()) else {
                    return Ok(Frame::Integer(-2));
                };
                let Some(deadline) = entry.deadline else {
                    return Ok(Frame::Integer(-1));
                };
                let milliseconds = deadline.saturating_duration_since(now).as_millis();
                // Redis TTL rounds remaining integral milliseconds to nearest second.
                let value = if command.name == "ttl" {
                    (milliseconds + 500) / 1000
                } else {
                    milliseconds
                };
                Ok(Frame::Integer(i64::try_from(value).unwrap_or(i64::MAX)))
            }
            "persist" => {
                self.touch(&args[1], now);
                if !self
                    .entries
                    .get(args[1].as_ref())
                    .is_some_and(|e| e.deadline.is_some())
                {
                    return Ok(Frame::Integer(0));
                }
                if let Some((key, entry)) = self.take(&args[1]) {
                    self.put_owned(key, entry.value, None, entry.cost - EXPIRY_OVERHEAD);
                }
                Ok(Frame::Integer(1))
            }
            "dbsize" => Ok(integer_size(self.live_count(now))),
            "info" => Ok(self.info(args, now)),
            "flushdb" => {
                if args.len() > 2 {
                    return Err(CommandError::Arity("flushdb"));
                }
                if args.len() == 2 && !args[1].eq_ignore_ascii_case(b"sync") {
                    return Err(CommandError::SyncOnly);
                }
                // Drop retained capacity as well as entries on an explicit flush.
                self.entries = HashMap::new();
                self.expirations = BTreeSet::new();
                self.used = 0;
                Ok(ok())
            }
            "command" => command_command(args),
            _ => Err(CommandError::ConnectionOnly),
        }
    }

    fn set(&mut self, args: &[Bytes], now: Instant) -> CommandResult {
        let mut nx = false;
        let mut xx = false;
        let mut get = false;
        let mut keep_ttl = false;
        let mut deadline = None;
        let mut index = 3;
        while let Some(option) = args.get(index) {
            if option.eq_ignore_ascii_case(b"nx") {
                nx = true;
            } else if option.eq_ignore_ascii_case(b"xx") {
                xx = true;
            } else if option.eq_ignore_ascii_case(b"get") {
                get = true;
            } else if option.eq_ignore_ascii_case(b"keepttl") {
                keep_ttl = true;
            } else if option.eq_ignore_ascii_case(b"ex") || option.eq_ignore_ascii_case(b"px") {
                if deadline.is_some() {
                    return Err(CommandError::Syntax);
                }
                index += 1;
                let raw = args.get(index).ok_or(CommandError::Syntax)?;
                deadline = Some(parse_deadline(
                    raw,
                    option.eq_ignore_ascii_case(b"ex"),
                    now,
                    true,
                )?);
            } else {
                return Err(CommandError::Syntax);
            }
            index += 1;
        }
        if (nx && xx) || (keep_ttl && deadline.is_some()) {
            return Err(CommandError::Syntax);
        }
        let key = &args[1];
        self.touch(key, now);
        let previous = if get {
            self.value_frame(key)
        } else {
            Frame::Null
        };
        let existing = self.entries.get(key.as_ref());
        if (nx && existing.is_some()) || (xx && existing.is_none()) {
            return Ok(previous);
        }
        if keep_ttl {
            deadline = existing.and_then(|entry| entry.deadline);
        }
        let removed = existing.map_or(0, |entry| entry.cost);
        let cost = entry_memory_cost(key.len(), args[2].len(), deadline.is_some())
            .ok_or(CommandError::Oom)?;
        self.admit(removed, cost, now)?;
        self.reserve_key(key)?;
        self.put_owned(
            Bytes::copy_from_slice(key),
            Bytes::copy_from_slice(&args[2]),
            deadline,
            cost,
        );
        Ok(if get { previous } else { ok() })
    }

    fn mset(&mut self, args: &[Bytes], now: Instant) -> CommandResult {
        if args.len() % 2 != 1 {
            return Err(CommandError::Arity("mset"));
        }
        // Borrowed staging deduplicates before costing or copying; last value wins.
        let mut unique = HashMap::new();
        unique
            .try_reserve(args.len() / 2)
            .map_err(|_| CommandError::Oom)?;
        for [key, value] in args[1..].as_chunks::<2>().0 {
            unique.insert(key.as_ref(), value);
        }
        let mut removed = 0usize;
        let mut added = 0usize;
        let mut new_keys = 0usize;
        for (key, value) in &unique {
            self.touch(key, now);
            let old_cost = self.entries.get(*key).map_or(0, |entry| entry.cost);
            removed = removed.checked_add(old_cost).ok_or(CommandError::Oom)?;
            added = added
                .checked_add(
                    entry_memory_cost(key.len(), value.len(), false).ok_or(CommandError::Oom)?,
                )
                .ok_or(CommandError::Oom)?;
            new_keys += usize::from(old_cost == 0);
        }
        self.admit(removed, added, now)?;
        self.entries
            .try_reserve(new_keys)
            .map_err(|_| CommandError::Oom)?;
        let mut staged = Vec::new();
        staged
            .try_reserve_exact(unique.len())
            .map_err(|_| CommandError::Oom)?;
        for (key, value) in unique {
            let cost = entry_memory_cost(key.len(), value.len(), false).ok_or(CommandError::Oom)?;
            staged.push((
                Bytes::copy_from_slice(key),
                Bytes::copy_from_slice(value),
                cost,
            ));
        }
        // Remove every old entry first so intermediate accounting cannot exceed
        // the validated final total due to hash iteration order.
        for (key, _, _) in &staged {
            self.take(key);
        }
        for (key, value, cost) in staged {
            self.put_owned(key, value, None, cost);
        }
        Ok(ok())
    }

    fn append(&mut self, args: &[Bytes], now: Instant) -> CommandResult {
        let key = &args[1];
        self.touch(key, now);
        let existing = self.entries.get(key.as_ref());
        let old_len = existing.map_or(0, |e| e.value.len());
        let length = old_len
            .checked_add(args[2].len())
            .ok_or(CommandError::Oom)?;
        let deadline = existing.and_then(|e| e.deadline);
        let removed = existing.map_or(0, |e| e.cost);
        let cost =
            entry_memory_cost(key.len(), length, deadline.is_some()).ok_or(CommandError::Oom)?;
        self.admit(removed, cost, now)?;
        self.reserve_key(key)?;
        let mut value = Vec::new();
        value
            .try_reserve_exact(length)
            .map_err(|_| CommandError::Oom)?;
        if let Some(entry) = self.entries.get(key.as_ref()) {
            value.extend_from_slice(&entry.value);
        }
        value.extend_from_slice(&args[2]);
        self.put_owned(
            Bytes::copy_from_slice(key),
            Bytes::from(value),
            deadline,
            cost,
        );
        Ok(integer_size(length))
    }

    fn counter(&mut self, args: &[Bytes], command: &str, now: Instant) -> CommandResult {
        let amount = if args.len() == 3 {
            parse_integer(&args[2]).ok_or(CommandError::Integer)?
        } else {
            1
        };
        // Redis rejects this decrement even when subtraction could fit: its
        // command contract requires a representable negated increment.
        if command == "decrby" && amount == i64::MIN {
            return Err(CommandError::Overflow);
        }
        let key = &args[1];
        self.touch(key, now);
        let existing = self.entries.get(key.as_ref());
        let old = match existing {
            Some(entry) => parse_integer(&entry.value).ok_or(CommandError::Integer)?,
            None => 0,
        };
        // Subtract directly: negating i64::MIN would itself overflow.
        let result = if command.starts_with("decr") {
            old.checked_sub(amount)
        } else {
            old.checked_add(amount)
        }
        .ok_or(CommandError::Overflow)?;
        let value = Bytes::from(result.to_string());
        let deadline = existing.and_then(|e| e.deadline);
        let removed = existing.map_or(0, |e| e.cost);
        let cost = entry_memory_cost(key.len(), value.len(), deadline.is_some())
            .ok_or(CommandError::Oom)?;
        self.admit(removed, cost, now)?;
        self.reserve_key(key)?;
        self.put_owned(Bytes::copy_from_slice(key), value, deadline, cost);
        Ok(Frame::Integer(result))
    }

    fn expire_command(&mut self, args: &[Bytes], seconds: bool, now: Instant) -> CommandResult {
        let deadline = parse_deadline(&args[2], seconds, now, false)?;
        let (mut nx, mut xx, mut gt, mut lt) = (false, false, false, false);
        for option in &args[3..] {
            if option.eq_ignore_ascii_case(b"nx") {
                nx = true;
            } else if option.eq_ignore_ascii_case(b"xx") {
                xx = true;
            } else if option.eq_ignore_ascii_case(b"gt") {
                gt = true;
            } else if option.eq_ignore_ascii_case(b"lt") {
                lt = true;
            } else {
                return Err(CommandError::Syntax);
            }
        }
        if (nx && (xx || gt || lt)) || (gt && lt) {
            return Err(CommandError::Syntax);
        }
        let key = &args[1];
        self.touch(key, now);
        let Some(entry) = self.entries.get(key.as_ref()) else {
            return Ok(Frame::Integer(0));
        };
        let previous = entry.deadline;
        // A persistent key has an infinite TTL for GT/LT comparisons.
        if (nx && previous.is_some())
            || (xx && previous.is_none())
            || (gt && previous.is_none_or(|old| deadline <= old))
            || (lt && previous.is_some_and(|old| deadline >= old))
        {
            return Ok(Frame::Integer(0));
        }
        if deadline <= now {
            self.take(key);
            return Ok(Frame::Integer(1));
        }
        let removed = entry.cost;
        let cost =
            entry_memory_cost(key.len(), entry.value.len(), true).ok_or(CommandError::Oom)?;
        self.admit(removed, cost, now)?;
        if let Some((key, entry)) = self.take(key) {
            self.put_owned(key, entry.value, Some(deadline), cost);
        }
        Ok(Frame::Integer(1))
    }

    /// Preflight a whole mutation; the target keys have already been lazy-expired.
    fn admit(&mut self, removed: usize, added: usize, now: Instant) -> Result<(), CommandError> {
        let fits = |used: usize| {
            used.checked_sub(removed)
                .and_then(|n| n.checked_add(added))
                .is_some_and(|n| n <= self.max_memory)
        };
        if fits(self.used) {
            return Ok(());
        }
        self.expire(now, OOM_EXPIRE_BUDGET);
        if self
            .used
            .checked_sub(removed)
            .and_then(|n| n.checked_add(added))
            .is_some_and(|n| n <= self.max_memory)
        {
            Ok(())
        } else {
            Err(CommandError::Oom)
        }
    }

    fn reserve_key(&mut self, key: &[u8]) -> Result<(), CommandError> {
        if !self.entries.contains_key(key) {
            self.entries.try_reserve(1).map_err(|_| CommandError::Oom)?;
        }
        Ok(())
    }

    fn touch(&mut self, key: &[u8], now: Instant) {
        if self
            .entries
            .get(key)
            .is_some_and(|e| e.deadline.is_some_and(|d| d <= now))
        {
            self.take(key);
        }
    }

    /// The only removal path; unlink the index immediately, including on replace.
    fn take(&mut self, key: &[u8]) -> Option<(Bytes, Entry)> {
        let (key, entry) = self.entries.remove_entry(key)?;
        if let Some(deadline) = entry.deadline {
            let removed = self.expirations.remove(&(deadline, key.clone()));
            debug_assert!(removed, "each expiring entry has exactly one index record");
        }
        self.used -= entry.cost;
        Some((key, entry))
    }

    /// Called only after checked admission and allocation of owned key/value bytes.
    fn put_owned(&mut self, key: Bytes, value: Bytes, deadline: Option<Instant>, cost: usize) {
        self.take(&key);
        if let Some(deadline) = deadline {
            self.expirations.insert((deadline, key.clone()));
        }
        // admit() checked the entire final total before any live-key mutation.
        self.used += cost;
        self.entries.insert(
            key,
            Entry {
                value,
                deadline,
                cost,
            },
        );
    }

    fn value_frame(&self, key: &[u8]) -> Frame {
        self.entries
            .get(key)
            .map_or(Frame::Null, |e| Frame::Bulk(e.value.clone()))
    }

    fn live_count(&self, now: Instant) -> usize {
        self.entries
            .values()
            .filter(|e| e.deadline.is_none_or(|d| d > now))
            .count()
    }

    fn info(&self, args: &[Bytes], now: Instant) -> Frame {
        let wanted = |section: &[u8]| {
            args.len() == 1
                || args[1..].iter().any(|s| {
                    s.eq_ignore_ascii_case(section)
                        || s.eq_ignore_ascii_case(b"all")
                        || s.eq_ignore_ascii_case(b"default")
                })
        };
        let mut output = String::new();
        if wanted(b"server") {
            output.push_str(concat!("# Server\r\nserver_name:remik\r\nremik_version:", env!("CARGO_PKG_VERSION"), "\r\nremik_mode:memory-only\r\nprotocol:resp2\r\ncompatibility:string-kv-subset\r\npersistence:none\r\n"));
        }
        if wanted(b"memory") {
            output.push_str(&format!("# Memory\r\nused_memory:{}\r\nmaxmemory:{}\r\nmaxmemory_policy:noeviction\r\nmemory_accounting:live-entry-estimate-not-rss\r\nentry_overhead:{}\r\nexpiry_overhead:{}\r\nexpiry_index_entries:{}\r\noom_expire_budget:{}\r\n", self.used, self.max_memory, ENTRY_OVERHEAD, EXPIRY_OVERHEAD, self.expirations.len(), OOM_EXPIRE_BUDGET));
        }
        if wanted(b"keyspace") {
            let expires = self.expirations.iter().filter(|(d, _)| *d > now).count();
            output.push_str(&format!(
                "# Keyspace\r\ndb0:keys={},expires={}\r\n",
                self.live_count(now),
                expires
            ));
        }
        Frame::Bulk(Bytes::from(output))
    }
}

type CommandResult = Result<Frame, CommandError>;

enum CommandError {
    Arity(&'static str),
    Syntax,
    Integer,
    Overflow,
    Expiry,
    Oom,
    Unknown,
    ConnectionOnly,
    SyncOnly,
}

impl CommandError {
    fn frame(self) -> Frame {
        let message = match self {
            Self::Arity(name) => {
                return Frame::Error(format!(
                    "ERR wrong number of arguments for '{name}' command"
                ));
            }
            Self::Syntax => "ERR syntax error",
            Self::Integer => "ERR value is not an integer or out of range",
            Self::Overflow => "ERR increment or decrement would overflow",
            Self::Expiry => "ERR invalid expire time",
            Self::Oom => "OOM command not allowed when used memory > 'maxmemory'",
            Self::Unknown => "ERR unknown command",
            Self::ConnectionOnly => "ERR command requires connection context",
            Self::SyncOnly => "ERR only synchronous FLUSHDB is supported",
        };
        Frame::Error(message.into())
    }
}

fn ok() -> Frame {
    Frame::Simple("OK".into())
}

fn integer_size(size: usize) -> Frame {
    // Native allocatable collections fit i64; saturation is defensive on wider targets.
    Frame::Integer(i64::try_from(size).unwrap_or(i64::MAX))
}

/// Redis-style canonical signed decimal: no plus, whitespace, leading zero or -0.
fn parse_integer(input: &[u8]) -> Option<i64> {
    if input == b"0" {
        return Some(0);
    }
    let (negative, digits) = match input.strip_prefix(b"-") {
        Some(digits) => (true, digits),
        None => (false, input),
    };
    if !matches!(digits.first(), Some(b'1'..=b'9')) || digits.len() > 19 {
        return None;
    }
    let mut value = 0i64;
    for &digit in digits {
        if !digit.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?;
        let digit = i64::from(digit - b'0');
        value = if negative {
            value.checked_sub(digit)?
        } else {
            value.checked_add(digit)?
        };
    }
    Some(value)
}

fn parse_deadline(
    input: &[u8],
    seconds: bool,
    now: Instant,
    positive: bool,
) -> Result<Instant, CommandError> {
    let value = parse_integer(input).ok_or(CommandError::Integer)?;
    let milliseconds = if seconds {
        value.checked_mul(1000).ok_or(CommandError::Expiry)?
    } else {
        value
    };
    if milliseconds <= 0 {
        return if positive {
            Err(CommandError::Expiry)
        } else {
            Ok(now)
        };
    }
    let milliseconds = u64::try_from(milliseconds).map_err(|_| CommandError::Expiry)?;
    now.checked_add(Duration::from_millis(milliseconds))
        .ok_or(CommandError::Expiry)
}

struct Command {
    name: &'static str,
    arity: i64,
    write: bool,
    first: i64,
    last: i64,
    step: i64,
}

impl Command {
    fn accepts(&self, count: usize) -> bool {
        if self.arity < 0 {
            count >= self.arity.unsigned_abs() as usize
        } else {
            count == self.arity as usize
        }
    }

    /// Legacy six-field RESP2 COMMAND metadata; no claims of ACL/RESP3 support.
    fn frame(&self) -> Frame {
        Frame::Array(vec![
            Frame::Bulk(Bytes::from_static(self.name.as_bytes())),
            Frame::Integer(self.arity),
            Frame::Array(if self.write {
                vec![Frame::Simple("write".into())]
            } else {
                vec![Frame::Simple("readonly".into())]
            }),
            Frame::Integer(self.first),
            Frame::Integer(self.last),
            Frame::Integer(self.step),
        ])
    }
}

// This is also the server's supported top-level command list. Subcommands and
// RESP/database-version restrictions are enforced by the connection dispatcher.
const COMMANDS: &[Command] = &[
    Command {
        name: "get",
        arity: 2,
        write: false,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "set",
        arity: -3,
        write: true,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "mget",
        arity: -2,
        write: false,
        first: 1,
        last: -1,
        step: 1,
    },
    Command {
        name: "mset",
        arity: -3,
        write: true,
        first: 1,
        last: -1,
        step: 2,
    },
    Command {
        name: "del",
        arity: -2,
        write: true,
        first: 1,
        last: -1,
        step: 1,
    },
    Command {
        name: "exists",
        arity: -2,
        write: false,
        first: 1,
        last: -1,
        step: 1,
    },
    Command {
        name: "type",
        arity: 2,
        write: false,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "append",
        arity: 3,
        write: true,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "strlen",
        arity: 2,
        write: false,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "incr",
        arity: 2,
        write: true,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "incrby",
        arity: 3,
        write: true,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "decr",
        arity: 2,
        write: true,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "decrby",
        arity: 3,
        write: true,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "expire",
        arity: -3,
        write: true,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "pexpire",
        arity: -3,
        write: true,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "ttl",
        arity: 2,
        write: false,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "pttl",
        arity: 2,
        write: false,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "persist",
        arity: 2,
        write: true,
        first: 1,
        last: 1,
        step: 1,
    },
    Command {
        name: "dbsize",
        arity: 1,
        write: false,
        first: 0,
        last: 0,
        step: 0,
    },
    Command {
        name: "info",
        arity: -1,
        write: false,
        first: 0,
        last: 0,
        step: 0,
    },
    Command {
        name: "flushdb",
        arity: -1,
        write: true,
        first: 0,
        last: 0,
        step: 0,
    },
    Command {
        name: "command",
        arity: -1,
        write: false,
        first: 0,
        last: 0,
        step: 0,
    },
    Command {
        name: "ping",
        arity: -1,
        write: false,
        first: 0,
        last: 0,
        step: 0,
    },
    Command {
        name: "echo",
        arity: 2,
        write: false,
        first: 0,
        last: 0,
        step: 0,
    },
    Command {
        name: "quit",
        arity: 1,
        write: false,
        first: 0,
        last: 0,
        step: 0,
    },
    Command {
        name: "auth",
        arity: -2,
        write: false,
        first: 0,
        last: 0,
        step: 0,
    },
    Command {
        name: "hello",
        arity: -1,
        write: false,
        first: 0,
        last: 0,
        step: 0,
    },
    Command {
        name: "select",
        arity: 2,
        write: false,
        first: 0,
        last: 0,
        step: 0,
    },
    Command {
        name: "client",
        arity: -2,
        write: false,
        first: 0,
        last: 0,
        step: 0,
    },
];

fn command_command(args: &[Bytes]) -> CommandResult {
    if args.len() == 1 || (args.len() == 2 && args[1].eq_ignore_ascii_case(b"info")) {
        return Ok(Frame::Array(COMMANDS.iter().map(Command::frame).collect()));
    }
    if args[1].eq_ignore_ascii_case(b"count") {
        return if args.len() == 2 {
            Ok(integer_size(COMMANDS.len()))
        } else {
            Err(CommandError::Arity("command|count"))
        };
    }
    if args[1].eq_ignore_ascii_case(b"info") {
        return Ok(Frame::Array(
            args[2..]
                .iter()
                .map(|name| {
                    COMMANDS
                        .iter()
                        .find(|c| name.eq_ignore_ascii_case(c.name.as_bytes()))
                        .map_or(Frame::Null, Command::frame)
                })
                .collect(),
        ));
    }
    Err(CommandError::Syntax)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn run(db: &mut Db, now: Instant, args: &[&str]) -> Frame {
        let args: Vec<Bytes> = args
            .iter()
            .map(|s| Bytes::copy_from_slice(s.as_bytes()))
            .collect();
        db.execute(&args, now)
    }

    fn bulk(value: &str) -> Frame {
        Frame::Bulk(Bytes::copy_from_slice(value.as_bytes()))
    }

    fn is_error(frame: Frame, prefix: &str) {
        assert!(
            matches!(&frame, Frame::Error(s) if s.starts_with(prefix)),
            "frame={frame:?}"
        );
    }

    fn check_invariants(db: &Db) {
        let expected_index: BTreeSet<_> = db
            .entries
            .iter()
            .filter_map(|(k, e)| e.deadline.map(|d| (d, k.clone())))
            .collect();
        assert_eq!(db.expirations, expected_index);
        let mut total = 0;
        for (key, entry) in &db.entries {
            assert_eq!(
                Some(entry.cost),
                entry_memory_cost(key.len(), entry.value.len(), entry.deadline.is_some())
            );
            total += entry.cost;
        }
        assert_eq!(db.memory_used(), total);
        assert!(total <= db.max_memory);
    }

    #[test]
    fn set_conditions_get_and_keep_ttl() {
        let now = Instant::now();
        let mut db = Db::new(100_000);
        assert_eq!(
            run(&mut db, now, &["SET", "k", "no", "XX", "GET"]),
            Frame::Null
        );
        assert_eq!(run(&mut db, now, &["GET", "k"]), Frame::Null);
        assert_eq!(
            run(
                &mut db,
                now,
                &["sEt", "k", "one", "nX", "GET", "PX", "2000"]
            ),
            Frame::Null
        );
        assert_eq!(
            run(&mut db, now, &["SET", "k", "two", "NX", "GET"]),
            bulk("one")
        );
        assert_eq!(run(&mut db, now, &["PTTL", "k"]), Frame::Integer(2000));
        assert_eq!(
            run(&mut db, now, &["SET", "k", "two", "XX", "GET", "KEEPTTL"]),
            bulk("one")
        );
        assert_eq!(run(&mut db, now, &["PTTL", "k"]), Frame::Integer(2000));
        assert_eq!(run(&mut db, now, &["GET", "k"]), bulk("two"));
        assert_eq!(
            run(&mut db, now, &["SET", "k", "three", "GET"]),
            bulk("two")
        );
        assert_eq!(run(&mut db, now, &["TTL", "k"]), Frame::Integer(-1));
        assert_eq!(run(&mut db, now, &["SET", "new", "v", "KEEPTTL"]), ok());
        assert_eq!(run(&mut db, now, &["TTL", "new"]), Frame::Integer(-1));
        check_invariants(&db);
    }

    #[test]
    fn invalid_set_is_atomic_even_when_condition_would_skip_write() {
        let now = Instant::now();
        let mut db = Db::new(100_000);
        assert_eq!(run(&mut db, now, &["SET", "k", "old", "EX", "10"]), ok());
        let used = db.memory_used();
        for options in [
            vec!["NX", "XX"],
            vec!["EX", "0"],
            vec!["PX", "-1"],
            vec!["EX"],
            vec!["EX", "1", "PX", "2"],
            vec!["PX", "1", "PX", "2"],
            vec!["PX", "1", "KEEPTTL"],
            vec!["KEEPTTL", "EX", "1"],
            vec!["NX", "EX", "9223372036854775807"],
            vec!["EX", "+1"],
            vec!["XX", "PX", "9223372036854775808"],
            vec!["GET", "unrecognized\r\n+OK"],
        ] {
            let mut args = vec!["SET", "k", "changed"];
            args.extend_from_slice(&options);
            is_error(run(&mut db, now, &args), "ERR");
            assert_eq!(
                run(&mut db, now, &["GET", "k"]),
                bulk("old"),
                "options={options:?}"
            );
            assert_eq!(run(&mut db, now, &["PTTL", "k"]), Frame::Integer(10_000));
            assert_eq!(db.memory_used(), used);
            check_invariants(&db);
        }
    }

    #[test]
    fn ttl_rounding_milliseconds_and_lazy_expiry() {
        let now = Instant::now();
        let mut db = Db::new(100_000);
        assert_eq!(run(&mut db, now, &["TTL", "k"]), Frame::Integer(-2));
        assert_eq!(run(&mut db, now, &["SET", "k", "v", "PX", "1500"]), ok());
        assert_eq!(run(&mut db, now, &["TTL", "k"]), Frame::Integer(2));
        let later = now + Duration::from_nanos(500_000);
        assert_eq!(run(&mut db, later, &["PTTL", "k"]), Frame::Integer(1499));
        assert_eq!(run(&mut db, later, &["TTL", "k"]), Frame::Integer(1));
        let later = now + Duration::from_millis(1001);
        assert_eq!(run(&mut db, later, &["TTL", "k"]), Frame::Integer(0));
        assert_eq!(run(&mut db, later, &["PTTL", "k"]), Frame::Integer(499));
        let later = now + Duration::from_millis(1500);
        assert_eq!(run(&mut db, later, &["GET", "k"]), Frame::Null);
        assert_eq!(run(&mut db, later, &["PTTL", "k"]), Frame::Integer(-2));
        assert_eq!(db.memory_used(), 0);
        check_invariants(&db);
    }

    #[test]
    fn expire_conditions_persist_and_nonpositive_deletion() {
        let now = Instant::now();
        let mut db = Db::new(100_000);
        assert_eq!(run(&mut db, now, &["SET", "k", "v"]), ok());
        assert_eq!(
            run(&mut db, now, &["EXPIRE", "k", "1", "GT"]),
            Frame::Integer(0)
        );
        assert_eq!(
            run(&mut db, now, &["EXPIRE", "k", "1", "XX"]),
            Frame::Integer(0)
        );
        assert_eq!(
            run(&mut db, now, &["EXPIRE", "k", "1", "NX"]),
            Frame::Integer(1)
        );
        assert_eq!(
            run(&mut db, now, &["EXPIRE", "k", "2", "NX"]),
            Frame::Integer(0)
        );
        assert_eq!(
            run(&mut db, now, &["EXPIRE", "k", "2", "GT", "XX"]),
            Frame::Integer(1)
        );
        assert_eq!(
            run(&mut db, now, &["EXPIRE", "k", "1", "LT", "XX"]),
            Frame::Integer(1)
        );
        assert_eq!(
            run(&mut db, now, &["EXPIRE", "k", "1", "LT"]),
            Frame::Integer(0)
        );
        for options in [["NX", "XX"], ["NX", "GT"], ["NX", "LT"], ["GT", "LT"]] {
            is_error(
                run(&mut db, now, &["EXPIRE", "k", "0", options[0], options[1]]),
                "ERR",
            );
            assert_eq!(run(&mut db, now, &["PTTL", "k"]), Frame::Integer(1000));
        }
        assert_eq!(
            run(&mut db, now, &["EXPIRE", "k", "0", "GT"]),
            Frame::Integer(0)
        );
        assert_eq!(run(&mut db, now, &["PERSIST", "k"]), Frame::Integer(1));
        assert_eq!(run(&mut db, now, &["PERSIST", "k"]), Frame::Integer(0));
        assert_eq!(run(&mut db, now, &["TTL", "k"]), Frame::Integer(-1));
        assert_eq!(
            run(&mut db, now, &["EXPIRE", "k", "-1", "LT"]),
            Frame::Integer(1)
        );
        assert_eq!(run(&mut db, now, &["PERSIST", "k"]), Frame::Integer(0));
        assert_eq!(run(&mut db, now, &["SET", "k", "v"]), ok());
        is_error(
            run(&mut db, now, &["EXPIRE", "k", "9223372036854775807"]),
            "ERR invalid expire",
        );
        is_error(
            run(&mut db, now, &["EXPIRE", "k", "-9223372036854775808"]),
            "ERR invalid expire",
        );
        assert_eq!(run(&mut db, now, &["GET", "k"]), bulk("v"));
        assert_eq!(
            run(&mut db, now, &["PEXPIRE", "k", "-9223372036854775808"]),
            Frame::Integer(1)
        );
        assert_eq!(
            run(&mut db, now, &["EXPIRE", "absent", "1"]),
            Frame::Integer(0)
        );
        is_error(run(&mut db, now, &["EXPIRE", "absent", "1", "bad"]), "ERR");
        check_invariants(&db);
    }

    #[test]
    fn counters_strict_signed_parsing_and_overflow() {
        for input in [
            "",
            "-0",
            "00",
            "01",
            "-01",
            "+1",
            " 1",
            "1 ",
            "1\0",
            "1.0",
            "9223372036854775808",
            "-9223372036854775809",
        ] {
            assert_eq!(parse_integer(input.as_bytes()), None, "input={input:?}");
        }
        let now = Instant::now();
        let mut db = Db::new(100_000);
        assert_eq!(run(&mut db, now, &["INCR", "k"]), Frame::Integer(1));
        assert_eq!(run(&mut db, now, &["DECRBY", "k", "2"]), Frame::Integer(-1));
        is_error(
            run(&mut db, now, &["DECRBY", "k", "-9223372036854775808"]),
            "ERR increment",
        );
        assert_eq!(run(&mut db, now, &["GET", "k"]), bulk("-1"));
        assert_eq!(
            run(&mut db, now, &["SET", "k", "9223372036854775807"]),
            ok()
        );
        is_error(run(&mut db, now, &["INCR", "k"]), "ERR increment");
        assert_eq!(
            run(&mut db, now, &["GET", "k"]),
            bulk("9223372036854775807")
        );
        assert_eq!(
            run(
                &mut db,
                now,
                &["SET", "k", "-9223372036854775808", "PX", "2000"]
            ),
            ok()
        );
        is_error(run(&mut db, now, &["DECR", "k"]), "ERR increment");
        assert_eq!(
            run(&mut db, now, &["INCRBY", "k", "9223372036854775807"]),
            Frame::Integer(-1)
        );
        assert_eq!(run(&mut db, now, &["PTTL", "k"]), Frame::Integer(2000));
        is_error(
            run(&mut db, now, &["DECRBY", "k", "-9223372036854775808"]),
            "ERR increment",
        );
        assert_eq!(run(&mut db, now, &["GET", "k"]), bulk("-1"));
        assert_eq!(run(&mut db, now, &["PTTL", "k"]), Frame::Integer(2000));
        is_error(run(&mut db, now, &["INCRBY", "missing", "01"]), "ERR value");
        assert_eq!(run(&mut db, now, &["GET", "missing"]), Frame::Null);
        assert_eq!(run(&mut db, now, &["SET", "bad", "-0"]), ok());
        is_error(run(&mut db, now, &["INCR", "bad"]), "ERR value");
        assert_eq!(run(&mut db, now, &["GET", "bad"]), bulk("-0"));
        check_invariants(&db);
    }

    #[test]
    fn mset_duplicates_and_errors_are_atomic_and_clear_ttl() {
        let now = Instant::now();
        let cost = entry_memory_cost(1, 1, false).unwrap();
        let mut db = Db::new(cost * 2);
        assert_eq!(
            run(
                &mut db,
                now,
                &[
                    "MSET",
                    "a",
                    "first-is-too-big-if-charged",
                    "a",
                    "1",
                    "b",
                    "2"
                ]
            ),
            ok()
        );
        assert_eq!(db.memory_used(), 2 * cost);
        assert_eq!(
            run(&mut db, now, &["MGET", "b", "a", "a", "missing"]),
            Frame::Array(vec![bulk("2"), bulk("1"), bulk("1"), Frame::Null])
        );
        is_error(
            run(&mut db, now, &["MSET", "a", "new", "missing-value"]),
            "ERR wrong number",
        );
        is_error(
            run(&mut db, now, &["MSET", "a", "new", "c", "extra"]),
            "OOM",
        );
        assert_eq!(
            run(&mut db, now, &["MGET", "a", "b", "c"]),
            Frame::Array(vec![bulk("1"), bulk("2"), Frame::Null])
        );
        check_invariants(&db);
        let mut db = Db::new(100_000);
        assert_eq!(run(&mut db, now, &["SET", "a", "v", "EX", "2"]), ok());
        assert_eq!(run(&mut db, now, &["MSET", "a", "1", "a", "2"]), ok());
        assert_eq!(run(&mut db, now, &["PTTL", "a"]), Frame::Integer(-1));
        assert_eq!(run(&mut db, now, &["GET", "a"]), bulk("2"));
        check_invariants(&db);
    }

    #[test]
    fn exact_memory_admission_shrink_delete_and_reads_after_oom() {
        let now = Instant::now();
        let cost = entry_memory_cost(1, 4, false).unwrap();
        assert_eq!(cost, ENTRY_OVERHEAD + 5);
        assert_eq!(entry_memory_cost(1, 4, true), Some(cost + EXPIRY_OVERHEAD));
        assert_eq!(entry_memory_cost(usize::MAX, 0, false), None);
        let mut db = Db::new(cost);
        assert_eq!(run(&mut db, now, &["SET", "k", "1234"]), ok());
        assert_eq!(db.memory_used(), cost);
        for args in [
            vec!["SET", "k", "12345"],
            vec!["APPEND", "k", "5"],
            vec!["INCRBY", "k", "10000"],
            vec!["PEXPIRE", "k", "1000"],
        ] {
            is_error(run(&mut db, now, &args), "OOM");
            assert_eq!(run(&mut db, now, &["GET", "k"]), bulk("1234"));
            assert_eq!(run(&mut db, now, &["TTL", "k"]), Frame::Integer(-1));
            assert_eq!(db.memory_used(), cost);
        }
        assert_eq!(run(&mut db, now, &["SET", "k", "1"]), ok());
        assert_eq!(db.memory_used(), cost - 3);
        assert_eq!(run(&mut db, now, &["DEL", "k"]), Frame::Integer(1));
        assert_eq!(db.memory_used(), 0);
        let mut db = Db::new(0);
        is_error(run(&mut db, now, &["SET", "", ""]), "OOM");
        is_error(run(&mut db, now, &["INCR", "k"]), "OOM");
        assert_eq!(run(&mut db, now, &["GET", "k"]), Frame::Null);
        check_invariants(&db);
    }

    #[test]
    fn rejected_writes_preserve_values_and_expiration_index() {
        let now = Instant::now();
        let cost = entry_memory_cost(1, 3, true).unwrap();
        let mut db = Db::new(2 * cost);
        for key in ["a", "b"] {
            assert_eq!(run(&mut db, now, &["SET", key, "old", "PX", "1000"]), ok());
        }
        let too_large = "v".repeat(2 * cost);
        is_error(run(&mut db, now, &["SET", "a", &too_large, "GET"]), "OOM");
        is_error(
            run(&mut db, now, &["MSET", "a", "new", "b", &too_large]),
            "OOM",
        );
        assert_eq!(
            run(&mut db, now, &["MGET", "a", "b"]),
            Frame::Array(vec![bulk("old"), bulk("old")])
        );
        for key in ["a", "b"] {
            assert_eq!(run(&mut db, now, &["PTTL", key]), Frame::Integer(1000));
        }
        assert_eq!(db.memory_used(), 2 * cost);
        check_invariants(&db);
    }

    #[test]
    fn mset_admits_the_final_aggregate_not_each_intermediate_write() {
        let now = Instant::now();
        let long = "v".repeat(100);
        let mut db = Db::new(2 * ENTRY_OVERHEAD + 2 + long.len());
        assert_eq!(run(&mut db, now, &["MSET", "a", &long, "b", ""]), ok());
        assert_eq!(db.memory_used(), db.max_memory);
        for _ in 0..20 {
            assert_eq!(run(&mut db, now, &["MSET", "a", "", "b", &long]), ok());
            assert_eq!(run(&mut db, now, &["MSET", "a", &long, "b", ""]), ok());
            check_invariants(&db);
        }
    }

    #[test]
    fn stored_bytes_do_not_pin_external_owners_or_larger_slices() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        struct Owner {
            bytes: Vec<u8>,
            dropped: Arc<AtomicBool>,
        }
        impl AsRef<[u8]> for Owner {
            fn as_ref(&self) -> &[u8] {
                &self.bytes
            }
        }
        impl Drop for Owner {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Relaxed);
            }
        }

        let now = Instant::now();
        let dropped = Arc::new(AtomicBool::new(false));
        let source = Bytes::from_owner(Owner {
            bytes: vec![b'x'; 10_000],
            dropped: dropped.clone(),
        });
        let mut db = Db::new(entry_memory_cost(1, 2, false).unwrap());
        let args = [
            Bytes::from_static(b"SET"),
            source.slice(0..1),
            source.slice(1..3),
        ];
        assert_eq!(db.execute(&args, now), ok());
        drop(args);
        drop(source);
        assert!(dropped.load(Ordering::Relaxed));
        assert_eq!(run(&mut db, now, &["GET", "x"]), bulk("xx"));
        check_invariants(&db);
    }

    #[test]
    fn admission_reclamation_is_bounded_and_retry_can_succeed() {
        let now = Instant::now();
        let cost = entry_memory_cost(4, 1, true).unwrap();
        let total = cost * (OOM_EXPIRE_BUDGET + 1);
        let mut db = Db::new(total);
        for n in 0..=OOM_EXPIRE_BUDGET {
            assert_eq!(
                run(&mut db, now, &["SET", &format!("{n:04}"), "v", "PX", "1"]),
                ok()
            );
        }
        assert_eq!(db.memory_used(), total);
        let later = now + Duration::from_millis(1);
        let large = "v".repeat(total - ENTRY_OVERHEAD - 4);
        is_error(run(&mut db, later, &["SET", "next", &large]), "OOM");
        assert_eq!(db.entries.len(), 1);
        assert_eq!(db.memory_used(), cost);
        check_invariants(&db);
        db.expire(later, 0);
        assert_eq!(db.entries.len(), 1);
        db.expire(later, 1);
        assert_eq!(db.memory_used(), 0);
        assert_eq!(run(&mut db, later, &["SET", "next", &large]), ok());
        assert_eq!(db.memory_used(), total);
        check_invariants(&db);
    }

    #[test]
    fn expiry_index_has_no_stale_records_and_background_budget_is_exact() {
        let now = Instant::now();
        let mut db = Db::new(100_000);
        for _ in 0..1000 {
            assert_eq!(run(&mut db, now, &["SET", "k", "v", "EX", "2"]), ok());
            assert_eq!(db.expirations.len(), 1);
            assert_eq!(
                run(&mut db, now, &["PEXPIRE", "k", "1000"]),
                Frame::Integer(1)
            );
            assert_eq!(db.expirations.len(), 1);
            assert_eq!(run(&mut db, now, &["PERSIST", "k"]), Frame::Integer(1));
            assert!(db.expirations.is_empty());
        }
        for key in ["a", "b", "c"] {
            assert_eq!(run(&mut db, now, &["SET", key, "v", "PX", "10"]), ok());
        }
        let later = now + Duration::from_millis(10);
        assert_eq!(run(&mut db, later, &["DBSIZE"]), Frame::Integer(1));
        assert_eq!(db.expirations.len(), 3); // counting does not run an unbounded expiry sweep.
        db.expire(later, 2);
        assert_eq!(db.expirations.len(), 1);
        check_invariants(&db);
        assert_eq!(run(&mut db, later, &["GET", "c"]), Frame::Null);
        assert!(db.expirations.is_empty());
        assert_eq!(run(&mut db, later, &["SET", "k", "v", "PX", "100"]), ok());
        assert_eq!(run(&mut db, later, &["DEL", "k"]), Frame::Integer(1));
        check_invariants(&db);
        assert_eq!(db.memory_used(), 0);
    }

    #[test]
    fn binary_strings_append_ttl_types_and_duplicate_key_counts() {
        let now = Instant::now();
        let mut db = Db::new(100_000);
        assert_eq!(
            db.execute(
                &[
                    Bytes::from_static(b"SET"),
                    Bytes::from_static(b"\0\xff"),
                    Bytes::from_static(b"\xff\r\n"),
                    Bytes::from_static(b"PX"),
                    Bytes::from_static(b"1000")
                ],
                now
            ),
            ok()
        );
        assert_eq!(
            db.execute(
                &[
                    Bytes::from_static(b"APPEND"),
                    Bytes::from_static(b"\0\xff"),
                    Bytes::from_static(b"\0")
                ],
                now
            ),
            Frame::Integer(4)
        );
        assert_eq!(
            db.execute(
                &[Bytes::from_static(b"GET"), Bytes::from_static(b"\0\xff")],
                now
            ),
            Frame::Bulk(Bytes::from_static(b"\xff\r\n\0"))
        );
        assert_eq!(
            db.execute(
                &[Bytes::from_static(b"PTTL"), Bytes::from_static(b"\0\xff")],
                now
            ),
            Frame::Integer(1000)
        );
        assert_eq!(run(&mut db, now, &["APPEND", "new", ""]), Frame::Integer(0));
        assert_eq!(
            run(&mut db, now, &["TYPE", "new"]),
            Frame::Simple("string".into())
        );
        assert_eq!(
            run(&mut db, now, &["TYPE", "none"]),
            Frame::Simple("none".into())
        );
        assert_eq!(
            run(&mut db, now, &["APPEND", "new", "abc"]),
            Frame::Integer(3)
        );
        assert_eq!(run(&mut db, now, &["STRLEN", "new"]), Frame::Integer(3));
        assert_eq!(run(&mut db, now, &["STRLEN", "none"]), Frame::Integer(0));
        assert_eq!(
            run(&mut db, now, &["EXISTS", "new", "new", "none"]),
            Frame::Integer(2)
        );
        assert_eq!(
            run(&mut db, now, &["DEL", "new", "new", "none"]),
            Frame::Integer(1)
        );
        check_invariants(&db);
    }

    #[test]
    fn flush_info_metadata_and_unknown_commands_are_truthful() {
        let now = Instant::now();
        let mut db = Db::new(12345);
        assert_eq!(run(&mut db, now, &["SET", "k", "v", "EX", "1"]), ok());
        is_error(
            run(&mut db, now, &["FLUSHDB", "ASYNC"]),
            "ERR only synchronous",
        );
        assert_eq!(run(&mut db, now, &["GET", "k"]), bulk("v"));
        let Frame::Bulk(info) = run(&mut db, now, &["INFO"]) else {
            panic!("bulk INFO required")
        };
        let info = std::str::from_utf8(&info).unwrap();
        assert!(info.contains("server_name:remik\r\n"));
        assert!(info.contains("remik_version:0.1.0-rc.1\r\n"));
        assert!(info.contains("remik_mode:memory-only\r\n"));
        assert!(info.contains("maxmemory:12345\r\n"));
        assert!(!info.contains("redis_version:"));
        assert_eq!(run(&mut db, now, &["COMMAND", "COUNT"]), Frame::Integer(29));
        let Frame::Array(commands) = run(&mut db, now, &["COMMAND"]) else {
            panic!("array COMMAND required")
        };
        assert_eq!(commands.len(), 29);
        let names: BTreeSet<Bytes> = commands
            .into_iter()
            .map(|entry| {
                let Frame::Array(fields) = entry else {
                    panic!("array metadata required")
                };
                assert_eq!(fields.len(), 6);
                let Frame::Bulk(name) = &fields[0] else {
                    panic!("bulk name required")
                };
                name.clone()
            })
            .collect();
        let expected: BTreeSet<Bytes> = "get set mget mset del exists type append strlen incr incrby decr decrby expire pexpire ttl pttl persist dbsize info flushdb command ping echo quit auth hello select client".split(' ').map(|s| Bytes::copy_from_slice(s.as_bytes())).collect();
        assert_eq!(names, expected);
        let Frame::Array(metadata) = run(&mut db, now, &["COMMAND", "INFO", "GET", "AUTH", "KEYS"])
        else {
            panic!("array metadata required")
        };
        assert_eq!(metadata.len(), 3);
        assert_eq!(metadata[2], Frame::Null);
        is_error(run(&mut db, now, &["GET"]), "ERR wrong number");
        is_error(
            run(&mut db, now, &["unknown\r\n+OK"]),
            "ERR unknown command",
        );
        assert_eq!(run(&mut db, now, &["FLUSHDB", "SYNC"]), ok());
        assert_eq!(db.memory_used(), 0);
        assert!(db.expirations.is_empty());
        check_invariants(&db);
    }

    proptest! {
        #[test]
        fn canonical_integer_roundtrip(value in any::<i64>()) {
            prop_assert_eq!(parse_integer(value.to_string().as_bytes()), Some(value));
        }

        #[test]
        fn counter_arithmetic_matches_checked_operations(old in any::<i64>(), amount in any::<i64>(), subtract in any::<bool>()) {
            let now = Instant::now();
            let mut db = Db::new(100_000);
            prop_assert_eq!(run(&mut db, now, &["SET", "k", &old.to_string()]), ok());
            let expected = if subtract { amount.checked_neg().and_then(|delta| old.checked_add(delta)) } else { old.checked_add(amount) };
            let result = run(&mut db, now, &[if subtract { "DECRBY" } else { "INCRBY" }, "k", &amount.to_string()]);
            match expected {
                Some(value) => prop_assert_eq!(result, Frame::Integer(value)),
                None => {
                    prop_assert!(matches!(result, Frame::Error(_)));
                    prop_assert_eq!(run(&mut db, now, &["GET", "k"]), bulk(&old.to_string()));
                }
            }
            check_invariants(&db);
        }

        #[test]
        fn mutations_preserve_exact_accounting_and_index(ops in prop::collection::vec((0u8..10, 0u8..8, prop::collection::vec(any::<u8>(), 0..32)), 0..100)) {
            let mut now = Instant::now();
            let mut db = Db::new(12_000);
            for (operation, key, value) in ops {
                now += Duration::from_millis(1);
                let key = Bytes::from(vec![key]);
                let value = Bytes::from(value);
                let args = match operation {
                    0 => vec![Bytes::from_static(b"SET"), key, value],
                    1 => vec![Bytes::from_static(b"SET"), key, value, Bytes::from_static(b"PX"), Bytes::from_static(b"4")],
                    2 => vec![Bytes::from_static(b"APPEND"), key, value],
                    3 => vec![Bytes::from_static(b"PEXPIRE"), key, Bytes::from_static(b"3")],
                    4 => vec![Bytes::from_static(b"PERSIST"), key],
                    5 => vec![Bytes::from_static(b"DEL"), key],
                    6 => vec![Bytes::from_static(b"MSET"), key.clone(), value.clone(), key, value],
                    7 => vec![Bytes::from_static(b"INCR"), key],
                    8 => vec![Bytes::from_static(b"GET"), key],
                    _ => { db.expire(now, 2); Vec::new() }
                };
                let _ = db.execute(&args, now);
                check_invariants(&db);
            }
            db.expire(now + Duration::from_secs(1), usize::MAX);
            check_invariants(&db);
        }
    }
}
