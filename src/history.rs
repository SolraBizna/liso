use std::{
    fs::{File, remove_file, rename},
    io,
    io::{BufReader, BufRead, BufWriter, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

/// This keeps track of the most recent command lines entered.
pub struct History {
    /// The actual lines of history
    lines: Vec<String>,
    /// The maximum number of lines of history to keep. If `None`, keep
    /// unlimited.
    limit: Option<NonZeroUsize>,
    /// Whether to strip existing duplicates whenever we add new lines to the
    /// history.
    strip_duplicates: bool,
    autosave_handler: Option<Box<dyn Fn(&History) -> io::Result<()> + Send + Sync>>,
    autosave_interval: Option<NonZeroUsize>,
    lines_since_last_autosave: usize,
}

impl History {
    /// Create a new, empty History with default options.
    /// 
    /// The defaults are subject to change, but as of this version, they are:
    /// 
    /// - History limit: 100 lines
    /// - Strip duplicates: yes
    /// - No autosave handler
    /// - Autosave only on drop
    pub fn new() -> History {
        History {
            lines: vec![],
            limit: NonZeroUsize::new(100),
            strip_duplicates: true,
            autosave_handler: None,
            autosave_interval: None,
            lines_since_last_autosave: 0,
        }
    }
    /// Create a new History by reading the given file, with default options.
    /// 
    /// The defaults are subject to change, but as of this version, they are:
    /// 
    /// - History limit: 100 lines.
    /// - Strip duplicates: yes
    /// - Autosave to given file, carefully avoiding common pitfalls
    /// - Autosave only on drop
    pub fn from_file<P: AsRef<Path>>(path: P) -> io::Result<History> {
        let history_path = PathBuf::from(path.as_ref());
        let mut filename = match history_path.file_name() {
            Some(x) => x.to_owned(),
            None => return Err(io::Error::new(io::ErrorKind::Other, "history file had no filename")),
        };
        filename.push("^");
        let mut build_path = history_path.clone();
        build_path.set_file_name(filename);
        let mut filename = match history_path.file_name() {
            Some(x) => x.to_owned(),
            None => return Err(io::Error::new(io::ErrorKind::Other, "history file had no filename")),
        };
        filename.push("~");
        let mut backup_path = history_path.clone();
        backup_path.set_file_name(filename);
        let mut ret = History::new();
        match ret.read_history_from(&history_path) {
            Ok(0) => {
                // if the file is blank, assume it was truncated and maybe try
                // to load the backup
                match ret.read_history_from(&backup_path) {
                    // Got some lines! (Or no lines. Either way.)
                    Ok(x) => Ok(x),
                    // No backup file. Return success on the blank file. It
                    // could be legit!
                    Err(x) if x.kind() == io::ErrorKind::NotFound => Ok(0),
                    // Some other error, report.
                    Err(x) => Err(x),
                }
            },
            Ok(x) => Ok(x),
            Err(x) if x.kind() == io::ErrorKind::NotFound => {
                match ret.read_history_from(&backup_path) {
                    Ok(x) => Ok(x),
                    // ignore an error from the history simply not existing...
                    // we have to start from somewhere!
                    Err(x) if x.kind() == io::ErrorKind::NotFound => {
                        Ok(0)
                    },
                    // Return the error for reading the original
                    Err(_) => Err(x),
                }
            },
            Err(x) => Err(x),
        }?;
        let handler = Box::new(move |history: &History| -> io::Result<()> {
            history.write_history_to(&build_path)?;
            let _ = remove_file(&backup_path);
            rename(&history_path, &backup_path)?;
            rename(&build_path, &history_path)?;
            let _ = remove_file(&backup_path);
            Ok(())
        });
        ret.autosave_handler = Some(handler);
        Ok(ret)
    }
    /// Attempts to read history from the given file. Does not change any
    /// settings. Overwrites all current history. Returns the number of lines
    /// read.
    pub fn read_history_from<P: AsRef<Path>>(&mut self, path: P) -> io::Result<usize> {
        let f = File::open(path)?;
        let mut new_history = Vec::new();
        for l in BufReader::new(f).lines() {
            let mut l = l?;
            if new_history.is_empty() && l.starts_with("\u{FEFF}") {
                l.remove(0);
            }
            while l.ends_with("\r") { l.pop(); }
            new_history.push(l);
        }
        let ret = new_history.len();
        self.lines = new_history;
        Ok(ret)
    }
    /// Attempts to write history to the given file. Doesn't have any special
    /// logic for removing the file on write error, or backing up the original
    /// file, or et cetera. If you didn't create your `History` using
    /// `from_file`, that's up to you to build *using* this function.
    pub fn write_history_to<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let mut f = BufWriter::new(File::create(path)?);
        for line in self.lines.iter() {
            f.write_all(line.as_bytes())?;
            f.write_all(b"\n")?;
        }
        drop(f);
        Ok(())
    }
    /// Sets the maximum number of lines that will be saved in the history. If
    /// more lines than this are added, the oldest lines will be removed. This
    /// is a linear time operation, so don't set this to an absurdly large
    /// value!
    /// 
    /// This will not take effect until the next time a new history item is
    /// added.
    pub fn set_limit(&mut self, limit: Option<NonZeroUsize>) -> &mut History {
        self.limit = limit;
        self
    }
    /// If true (default), whenever a new line is added to the history, any
    /// existing copies of that line in the history will be deleted.
    /// 
    /// Existing duplicates will still not be removed unless new copies of
    /// those exact lines are added.
    pub fn set_strip_duplicates(&mut self, strip_duplicates: bool) -> &mut History {
        self.strip_duplicates = strip_duplicates;
        self
    }
    /// Sets an autosave handler, to be called when the `History` is about to
    /// be dropped, or on the autosave interval if configured. The old
    /// autosave handler, if any, is dropped.
    pub fn set_autosave_handler(&mut self, autosave_handler: Option<Box<dyn Fn(&History) -> io::Result<()> + Send + Sync>>) -> &mut History {
        self.autosave_handler = autosave_handler;
        self
    }
    /// Sets an autosave interval. With interval `Some(N)`, an autosave will be
    /// attempted after every `N` lines are added to the history. With
    /// interval `None`, autosaving will only be performed when the `History`
    /// is dropped.
    pub fn set_autosave_interval(&mut self, autosave_interval: Option<NonZeroUsize>) -> &mut History {
        if autosave_interval.is_none() {
            self.lines_since_last_autosave = 0;
        }
        self.autosave_interval = autosave_interval;
        self
    }
    /// Add a new line to the end of the history.
    /// 
    /// If duplicates are being stripped, will strip identical copies of this
    /// line from the history. If there is a limit on the number of lines in
    /// the history, this may remove the oldest history elements. If there is
    /// an autosave handler *and* an autosave interval, this may autosave the
    /// history.
    /// 
    /// Returns an error if autosaving failed.
    pub fn add_line(&mut self, line: String) -> io::Result<()> {
        if self.strip_duplicates {
            // wish drain_filter were stable
            for x in (0 .. self.lines.len()).rev() {
                if self.lines[x] == line { self.lines.remove(x); }
            }
        }
        if let Some(limit) = self.limit {
            let limit = limit.get() - 1;
            if self.lines.len() > limit {
                self.lines.splice(0 .. (self.lines.len() - limit), None);
            }
        }
        self.lines.push(line);
        if let Some(interval) = self.autosave_interval {
            self.lines_since_last_autosave += 1;
            if self.lines_since_last_autosave >= interval.get() {
                self.lines_since_last_autosave = 0;
                if let Some(autosave_handler) = self.autosave_handler.as_ref() {
                    (autosave_handler)(self)?;
                }
            }
        }
        Ok(())
    }
    /// Returns all the lines currently in the history.
    pub fn get_lines(&self) -> &[String] {
        &self.lines
    }
}
