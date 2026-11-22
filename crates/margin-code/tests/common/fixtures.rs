//! Invented fixtures shared by the outline and robustness tests.


pub const C: &str = r#"#include <stdio.h>
#include <string.h>

#define SQUARE(x) ((x) * (x))
#define LOG(fmt, ...) \
    fprintf(stderr, fmt, __VA_ARGS__)

static int parse_header(const char *in, size_t n);   /* prototype: not a definition */
typedef int (*handler_fn)(int code);
static handler_fn handlers[4];

struct point { int x; int y; };

static int
parse_header(const char *in, size_t n)
{
    char buf[32];
    /* strcpy(buf, in) in a comment */
    const char *msg = "memcpy(not, a, call)";
    if (n > sizeof(buf)) {
        return -1;
    }
    memcpy(buf, in, n);
    for (int i = 0; i < 3; i++) { buf[i] = SQUARE(i); }
    return helper(buf);
}

int run_handler(int code) {
    handler_fn h = handlers[code & 3];
    return (*h)(code);
}

#ifdef _WIN32
int spawn_shell(const char *cmd) {
#else
int spawn_shell(const char *cmd, int flags) {
#endif
    return system(cmd);
}

int main(int argc, char **argv)
{
    struct point p = { .x = 1, .y = 2 };
    printf("%d\n", parse_header(argv[1], strlen(argv[1])));
    return spawn_shell(argv[0]);
}
"#;

pub const CPP: &str = r#"#include <vector>
namespace geo {
template <typename T>
T clamp(T v, T lo, T hi) {
    return std::max(lo, std::min(v, hi));
}

class Shape {
public:
    Shape() : id_(next_id()) {}
    virtual ~Shape();
    virtual double area() const = 0;
    int id() const { return id_; }
private:
    int id_;
};

Shape::~Shape() {
    log_destroy(id_);
}

double Circle::area() const {
    auto sq = [](double r) { return r * r; };
    return 3.14 * sq(radius_);
}

bool operator==(const Shape& a, const Shape& b) { return a.id() == b.id(); }
}  // namespace geo

extern "C" {
int c_entry(char *dst, const char *src) {
    strcpy(dst, src);
    return std::system(dst);
}
}
"#;

pub const RUST: &str = r#"use std::process::Command;

/// Runs `fn fake()` in a doc comment.
pub struct Runner { name: String }

impl Runner {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string() }
    }

    #[inline]
    pub(crate) fn run(&self, arg: &str) -> std::io::Result<()> {
        let add = |a: i32, b: i32| -> i32 { compute(a, b) };
        let s = "Command::new(not a call)";
        Command::new("sh").arg(arg).status()?;
        println!("{}", add(1, 2));
        Ok(())
    }
}

trait Visit {
    fn visit(&self);
    fn twice(&self) {
        self.visit();
        self.visit();
    }
}

fn outer() -> u8 {
    fn inner(x: u8) -> u8 { x.wrapping_add(1) }
    let v: Vec<u8> = Vec::with_capacity(4);
    unsafe { std::mem::transmute::<u32, [u8; 4]>(0) };
    inner(v.len() as u8)
}
"#;

pub const GO: &str = r#"package main

import (
	"database/sql"
	"os/exec"
)

type Server struct{ db *sql.DB }

func (s *Server) Lookup(name string) error {
	rows, err := s.db.Query("SELECT * FROM t WHERE n = '" + name + "'")
	if err != nil {
		return err
	}
	defer rows.Close()
	go func() {
		notify(name)
	}()
	return nil
}

func run(cmd string) ([]byte, error) {
	return exec.Command("sh", "-c", cmd).Output()
}

type Greeter interface {
	Greet(name string) string
}

func Map[T any](xs []T, f func(T) T) []T {
	out := make([]T, 0, len(xs))
	for _, x := range xs {
		out = append(out, f(x))
	}
	return out
}
"#;

pub const JAVA: &str = r#"package demo;

import java.io.*;

@Service
public class UserStore<T extends Comparable<T>> implements Store {
    private final Map<String, T> cache = new HashMap<>();

    public UserStore() {
        super();
        init("x");
    }

    @Override
    public <R> List<R> find(String q) throws IOException {
        Statement st = conn.createStatement();
        ResultSet rs = st.executeQuery("select * from u where n='" + q + "'");
        Runtime.getRuntime().exec(q);
        return new ArrayList<R>();
    }

    void listen() {
        Runnable r = new Runnable() {
            @Override
            public void run() {
                handle(new ObjectInputStream(in).readObject());
            }
        };
        if (r != null) { r.run(); }
    }

    abstract void hook(int x);
}
"#;

pub const JS: &str = r#"'use strict';
const cp = require('child_process');

function build(cmd) {
  const s = "exec(not a call)"; // eval(nope)
  return cp.exec(cmd, (err, out) => log(out));
}

const handler = async (req, res) => {
  const data = await fetch(req.url);
  res.send(data);
};

const square = x => x * compute(x);

class Widget extends Base {
  constructor(el) {
    super(el);
    this.el = el;
  }
  render(html) {
    this.el.insertAdjacentHTML('beforeend', html);
    if (html) { document.write(html); }
  }
  static create() { return new Widget(document.body); }
  onClick = (e) => { this.render(e.target.value); };
}

module.exports = {
  helper: function (a) { return eval(a); },
  short(a) { return a + 1; },
};

export default function () { return build('ls'); }
"#;

pub const TS: &str = r#"import { Injectable } from '@angular/core';

interface Repo { find(id: string): Promise<User>; }

@Injectable()
export class UserService implements Repo {
  constructor(private http: HttpClient) {}

  async find(id: string): Promise<User> {
    const rows = await this.db.query(`SELECT * FROM u WHERE id = ${id}`);
    return rows[0] as User;
  }

  private parse<T>(raw: string): T {
    return JSON.parse(raw) as T;
  }
}

export const Panel: React.FC<Props> = (props) => {
  return render(props.children);
};

export function generic<T>(x: T): T { return identity<T>(x); }
"#;

pub const PYTHON: &str = r#"import os
import pickle
import subprocess


def plain(a, b=len("x")):
    """Docstring with call(1) inside."""
    total = a + b  # comment(2)
    return total


@app.route("/run",
           methods=["POST"])
@login_required
def run_cmd(request):
    cmd = request.form["cmd"]
    os.system(cmd)

    def nested(x):
        return eval(x)

    subprocess.run(cmd, shell=True)
    return nested(cmd)


class Store(Base):
    def load(self, blob):
        obj = pickle.loads(blob)
        self.cursor.execute("SELECT 1")
        return obj

    async def fetch(self, url):
        return await requests.get(url)

lam = lambda q: open(q)
"#;

pub const CSHARP: &str = r#"using System.Diagnostics;

namespace App.Services
{
    [Serializable]
    public class Runner : IRunner
    {
        public Runner(ILogger log) { _log = log; }

        public int Start(string cmd)
        {
            var p = Process.Start("cmd.exe", "/c " + cmd);
            var bf = new BinaryFormatter();
            var obj = bf.Deserialize(stream);
            return p.ExitCode;
        }

        public string Name => Compute(_name);

        public int Twice(int x) => Double(x) * 2;

        private async Task<int> QueryAsync(SqlCommand c)
        {
            return (int)await c.ExecuteScalarAsync();
        }
    }
}
"#;

pub const PHP: &str = r#"<?php
namespace App;

function render_page($tpl) {
    $html = file_get_contents($tpl);
    include($tpl . '.php');
    return str_replace('{x}', 'y', $html);
}

class Ctl extends Base {
    public function index($req) {
        $id = $req->get('id');
        $rows = $this->db->query("SELECT * FROM t WHERE id=$id");
        system('ls ' . $id);
        $fn = function ($x) use ($id) { return unserialize($x); };
        return $fn($rows);
    }

    private static function helper(): void {
        self::other();
    }
}
"#;

pub const RUBY: &str = r#"require 'yaml'

module Admin
  class UsersController < ApplicationController
    before_action :auth

    def index
      @users = User.where("name = '#{params[:q]}'")
      system "ls #{params[:dir]}" if params[:dir]
      render json: @users
    end

    def self.build(opts = {})
      data = YAML.load(opts[:raw])
      Marshal.load(data)
    end

    def total = items.sum(&:price)

    def update
      items.each do |i|
        i.save!
      end
      while running? do
        tick
      end
      unless ok?
        log("bad")
      end
    end
  end
end
"#;

pub const KOTLIN: &str = r#"package demo

class Shell(private val env: Map<String, String>) {
    fun run(cmd: String): Int {
        val p = Runtime.getRuntime().exec(cmd)
        listOf(1, 2).forEach { log(it) }
        return p.waitFor()
    }

    fun <T> List<T>.second(): T = this[1]

    override fun toString() = describe(env)

    constructor(raw: String) : this(parse(raw)) {
        check(raw)
    }
}

fun main(args: Array<String>) {
    Shell(emptyMap()).run(args[0])
}
"#;

pub const SWIFT: &str = r#"import Foundation

protocol Runner {
    func run(_ cmd: String) -> Int32
}

final class Task: Runner {
    let name: String

    init(name: String) {
        self.name = name
        setup()
    }

    func run(_ cmd: String) -> Int32 {
        let p = Process()
        p.launch()
        return system(cmd)
    }

    var computed: Int { return compute(1) }
}
"#;

pub const LUA: &str = r#"local M = {}

local function helper(x)
  return tostring(x)
end

function M.run(cmd)
  if cmd then
    os.execute(cmd)
  end
  for i = 1, 3 do
    helper(i)
  end
  return io.popen(cmd)
end

function M:method(a)
  local f = loadstring(a)
  repeat
    f()
  until true
end

M.cb = function(v)
  print "callback"
  return v
end

return M
"#;

pub const SHELL: &str = r#"#!/bin/sh
set -eu

log() {
  echo "$@" >&2
}

function deploy {
  log "deploying $1"
  curl -fsSL "$URL" -o /tmp/x
  eval "$CMD"
}

build () {
  make all && log done
}

deploy "$@"
"#;
