extern crate rustc_serialize;

pub mod columns;
pub mod error;

use self::columns::{Columns, BytesColumns};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::iter::Iterator;
use std::path::Path;

use error::{Error, Result};
use rustc_serialize::Decodable;

#[cfg(test)] mod test;

/// Csv reader
/// 
/// Iterates over the rows of the csv
///
/// # Example
///
/// ```rust
/// let csv = quick_csv::Csv::from_file("./examples/data/bench.csv").unwrap();
/// for row in csv.into_iter() {
///     let row = row.unwrap(); // unwrap result, panic if not utf8 
///     {
///         // either use columns iterator directly (Item = &str)
///         if let Ok(mut columns) = row.columns() {
///             println!("Column 1: '{:?}', Column 2: '{:?}'", columns.next(), columns.next());
///         }
///     }
///
///     {
///         // or decode it directly into something simpler
///         if let Ok((col1, col2)) = row.decode::<(String, u64)>() {
///             println!("Column 1: '{:?}', Column 2: '{:?}'", &col1, &col2);
///         }
///     }
///
/// }
/// ```
pub struct Csv<B: BufRead> {
    /// delimiter
    delimiter: u8,
    /// reader
    reader: B,
    /// header
    has_headers: bool,
    /// header
    headers: Option<Vec<String>>,
    /// flexible column count
    flexible: bool,
    /// column count
    len: Option<usize>,
    /// if was error, exit next
    exit: bool,
}

impl<B: BufRead> Csv<B> {

    /// Creates a Csv from a generic BufReader
    /// 
    /// Note: default delimiter = ','
    pub fn from_reader(reader: B) -> Csv<B> {
        Csv {
            reader: reader,
            delimiter: b',',
            has_headers: false,
            headers: None,
            flexible: false,
            len: None,
            exit: false,
        }
    }

    /// Sets a new delimiter
    pub fn delimiter(mut self, delimiter: u8) -> Csv<B> {
        self.delimiter = delimiter;
        self
    }

    /// Sets flexible columns
    pub fn flexible(mut self, flexible: bool) -> Csv<B> {
        self.flexible = flexible;
        self
    }

    /// Defines whether there is a header or not
    pub fn has_header(mut self, has_headers: bool) -> Csv<B> {
        self.has_headers = has_headers;
        let _ = self.headers();
        self
    }

   /// gets first row as Vec<String>
    pub fn headers(&mut self) -> Vec<String> {
        if let Some(ref h) = self.headers {
            return h.clone();
        }
        if self.has_headers {            
            if let Some(r) = self.next() {
                if let Ok(r) = r {
                    let h = r.decode().unwrap_or(Vec::new());
                    self.headers = Some(h.clone());
                    return h;
                }
            }
        }
        Vec::new()
    }

    /// Get column count
    pub fn len(&self) -> Option<usize> {
        self.len
    }
}

impl Csv<BufReader<File>> {
    /// Creates a csv from a file path
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Csv<BufReader<File>>>
    {
        let reader = BufReader::new(try!(File::open(path)));
        Ok(Csv::from_reader(reader))
    }
}

impl<'a> Csv<&'a [u8]> {
    /// Creates a CSV reader for an in memory string buffer.
    pub fn from_string(s: &'a str) -> Csv<&'a [u8]> {
        Csv::from_reader(s.as_bytes())
    }
}


/// Iterator on csv returning rows
impl<B: BufRead> Iterator for Csv<B> {
    type Item = Result<Row>;
    fn next(&mut self) -> Option<Result<Row>> {
        if self.exit { return None; }
        let mut buf = Vec::new();
        let mut cols = self.len.map_or_else(|| Vec::new(), |n| Vec::with_capacity(n));
        match read_line(&mut self.reader, &mut buf, self.delimiter, &mut cols) {
            Ok(0) => None,
            Ok(_n) => {
                if buf.ends_with(&[b'\r']) {
                    buf.pop();
                }
                cols.push(buf.len());
                let c = cols.len();
                if let Some(n) = self.len {
                    if n != c && !self.flexible {
                        return Some(Err(Error::ColumnMismatch(n, c)));
                    }
                } else {
                    self.len = Some(c);
                }
                Some(Ok(Row {
                    line: buf,
                    cols: cols,
                }))
            }
            Err(e) => {
                self.exit = true;
                Some(Err(e))
            },
        }
    }
}

/// Row struct used as Csv iterator Item
///
/// Row can be decoded into a Result<T: Decodable>
pub struct Row {
    line: Vec<u8>,
    cols: Vec<usize>,
}

impl Row {

    /// Gets an iterator over columns
    pub fn columns<'a>(&'a self) -> Result<Columns<'a>> {
        match ::std::str::from_utf8(&self.line) {
            Err(_) => Err(Error::from(io::Error::new(io::ErrorKind::InvalidData,
                                            "stream did not contain valid UTF-8"))),
            Ok(s) => Ok(Columns::new(s, &self.cols)),
        }
    }

    ///  Creates a new BytesColumns iterator over &[u8]
    pub fn bytes_columns<'a>(&'a self) -> BytesColumns<'a> {
        BytesColumns::new(&self.line, &self.cols)
    }

    /// Decode row into custom decodable type
    pub fn decode<T: Decodable>(&self) -> Result<T> {
        let mut columns = try!(self.columns());
        Decodable::decode(&mut columns)
    }

    /// Gets columns count
    pub fn len(&self) -> usize {
        self.cols.len()
    }

}

/// Consumes bytes as long as they are within quotes
/// manages "" as quote escape
/// returns
/// - Ok(true) if entirely consumed
/// - Ok(false) if no issue but it reached end of buffer
/// - Err(Error::UnescapeQuote) if a quote if found within the column
macro_rules! consume_quote {
    ($bytes: expr, $delimiter: expr, $in_quote: expr) => {
        $in_quote = false;
        loop {
            match $bytes.next() {
                Some((_, &b'\"')) => {
                    match $bytes.clone().next() {
                        Some((_, &b'\"')) => {
                            $bytes.next(); // escaping quote
                        },
                        None | Some((_, &b'\r')) | Some((_, &b'\n')) => break,
                        Some((_, d)) if *d == $delimiter => break,
                        Some((_, _)) => return Err(Error::UnescapedQuote),
                    }
                },
                None => {
                    $in_quote = true;
                    break;
                },
                _ => (),
            }
        }
    }
}

fn read_line<R: BufRead>(r: &mut R, buf: &mut Vec<u8>,
    delimiter: u8, cols: &mut Vec<usize>) -> Result<usize>
{
    let mut read = 0;
    let mut in_quote = false;
    let mut done = false;
    while !done {
        let used = {
            let available = match r.fill_buf() {
                Ok(n) if n.is_empty() => return Ok(read),
                Ok(n) => n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::from(e)),
            };
            
            let mut bytes = available.iter().enumerate();

            // previous buffer was exhausted without exiting from quotes
            if in_quote {
                consume_quote!(bytes, delimiter, in_quote);
            }

            // use a simple loop instead of for loop to allow nested loop
            let used: usize;
            loop {
                match bytes.next() {
                    Some((i, &b'\"')) => {
                        if i == 0 || available[i - 1] == delimiter {
                            consume_quote!(bytes, delimiter, in_quote);
                        } else {
                            return Err(Error::UnexpextedQuote);
                        }
                    },
                    Some((i, &b'\n')) => {
                        let _ = buf.write(&available[..i]);
                        done = true;
                        used = i + 1;
                        break;
                    },
                    Some((i, &d)) => {
                        if d == delimiter { cols.push(read + i); }
                    },
                    None => {
                        let _ = buf.write(available);
                        used = available.len();
                        break;
                    },
                }
            }
            used
        };
        r.consume(used);
        read += used;
    }
    Ok(read)
}
