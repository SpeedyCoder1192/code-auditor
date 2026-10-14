# Deliberately vulnerable sample for the demo. The key is fake.
import sqlite3
import subprocess

API_KEY = "sk-live-9f8a7b6c5d4e3f2a1b0c"

def login(db, username, password):
    cur = db.cursor()
    cur.execute("SELECT * FROM users WHERE name = '" + username + "' AND pw = '" + password + "'")
    return cur.fetchone() is not None

def ping(host):
    return subprocess.run("ping -c 1 " + host, shell=True)

def average(xs):
    return sum(xs) / len(xs)
