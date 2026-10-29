# Third-Party Notices

## fdupes v2.4.0

The C09 duplicate-grouping algorithm in `crates/kernel/src/duplicate.rs` is adapted from fdupes v2.4.0 (commit `5e86f87042b6472a4871881435fae1cc438f12d0`):

- Source: https://github.com/adrianlopezroche/fdupes
- Original functions consulted: `checkmatch`, `registerpair`, `is_same_file`, `has_same_file`
- Adaptation: pure-function rewrite; no I/O, hashing, CLI, threading, logging or deletion code copied.

MIT License

Copyright (c) 1999-2022 Adrian Lopez

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
