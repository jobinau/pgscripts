#!/usr/bin/env python3
"""
pg_upgrade_link_check.py -- verify that pg_upgrade --link (-k) really hard-linked
the old cluster's relation files into the new data directory.

For every regular file in the old data directory it compares (st_dev, st_ino)
with the file at the same relative path in the new data directory:

  LINKED            same device + inode  -> one copy of the data on disk
  LINKED_ELSEWHERE  inode found in the new tree, but under a different path
  COPY              path exists in the new tree but is a separate inode
  MISSING           path does not exist in the new tree at all

COPY / MISSING are normal for everything pg_upgrade rebuilds or copies rather
than links (catalogs, control data, config, pg_wal, pg_xact, ...), so those are
classified as "expected" and reported separately from real findings.

Usage:
  ./pg_upgrade_link_check.py OLDDATADIR NEWDATADIR [options]
Example:
  ./pg_upgrade_link_check.py /home/postgres/pg16datadir/ /home/postgres/pg18datadir/ --csv /tmp/links.csv

Options:
  --tablespaces        also compare PG_<old>_* vs PG_<new>_* trees under pg_tblspc
  --details            list every file in every category, not just unexpected ones
  --limit N            max files listed per category (default 40, 0 = unlimited)
  --csv FILE           write the full per-file classification to FILE
  -q, --quiet          only print the summary

Exit status: 0 = no unexpected findings, 1 = unexpected findings, 2 = usage/IO error.
Run it as the postgres user (or root) so it can stat everything.
"""

import argparse
import csv
import os
import stat
import sys
from collections import defaultdict

# Relations with an OID below this are system catalogs; pg_upgrade dumps and
# recreates them in the new cluster instead of linking the files.
FIRST_NORMAL_OID = 16384

# Top-level directories whose contents pg_upgrade never hard-links: they are
# either copied, recreated by initdb, or runtime state of the new cluster.
SERVER_MANAGED_DIRS = {
    "pg_wal", "pg_xlog", "pg_xact", "pg_clog", "pg_multixact", "pg_subtrans",
    "pg_commit_ts", "pg_notify", "pg_serial", "pg_snapshots", "pg_stat",
    "pg_stat_tmp", "pg_logical", "pg_replslot", "pg_twophase", "pg_dynshmem",
    "log", "pg_log", "pg_upgrade_output.d",
}

CLUSTER_FILES = {
    "PG_VERSION", "postgresql.conf", "postgresql.auto.conf", "pg_hba.conf",
    "pg_ident.conf", "postmaster.pid", "postmaster.opts", "current_logfiles",
    "pg_internal.init", "backup_label", "backup_label.old", "tablespace_map",
    "recovery.signal", "standby.signal", "delete_old_cluster.sh",
    "analyze_new_cluster.sh", "update_extensions.sql",
}

PER_DB_FILES = {"PG_VERSION", "pg_filenode.map", "pg_internal.init"}


def human(nbytes):
    value = float(nbytes)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if abs(value) < 1024.0 or unit == "TiB":
            return "%.1f %s" % (value, unit) if unit != "B" else "%d B" % nbytes
        value /= 1024.0


def relfilenode_of(name):
    """Extract the relfilenode from a relation file name, or None."""
    base = name.split(".")[0]              # strip segment number: 16384.3
    for suffix in ("_fsm", "_vm", "_init"):  # strip fork suffixes
        if base.endswith(suffix):
            base = base[: -len(suffix)]
            break
    return int(base) if base.isdigit() else None


def expected_reason(rel, in_tablespace=False):
    """Return why this path is *expected* not to be hard-linked, else None."""
    parts = rel.split(os.sep)
    if "pgsql_tmp" in parts:
        return "temporary files"
    if parts[-1] in PER_DB_FILES:
        return "per-database metadata (rebuilt)"

    if not in_tablespace:
        top = parts[0]
        if top in SERVER_MANAGED_DIRS:
            return "server-managed directory (copied or recreated)"
        if top == "global":
            return "shared catalogs and control data (rebuilt)"
        if len(parts) == 1:
            if top in CLUSTER_FILES or top.endswith(".conf"):
                return "cluster configuration/control file"
            return None
        if top != "base":
            return None

    node = relfilenode_of(parts[-1])
    if node is not None and node < FIRST_NORMAL_OID:
        return "system catalog relation (rebuilt)"
    return None


def scan(root):
    """Collect regular files and symlinks under root, without following links."""
    files, links, errors = {}, {}, []

    def on_error(err):
        errors.append(err)

    for dirpath, dirnames, filenames in os.walk(root, onerror=on_error):
        relbase = os.path.relpath(dirpath, root)
        relbase = "" if relbase == "." else relbase
        keep = []
        for name in dirnames:
            full = os.path.join(dirpath, name)
            if os.path.islink(full):
                links[os.path.join(relbase, name)] = os.readlink(full)
            else:
                keep.append(name)
        dirnames[:] = keep

        for name in filenames:
            rel = os.path.join(relbase, name)
            full = os.path.join(dirpath, name)
            try:
                st = os.lstat(full)
            except OSError as err:
                errors.append(err)
                continue
            if stat.S_ISLNK(st.st_mode):
                links[rel] = os.readlink(full)
            elif stat.S_ISREG(st.st_mode):
                files[rel] = st
    return files, links, errors


def compare_trees(label, old_root, new_root, in_tablespace=False):
    old_files, _, old_err = scan(old_root)
    new_files, _, new_err = scan(new_root)

    new_by_inode = defaultdict(list)
    for rel, st in new_files.items():
        new_by_inode[(st.st_dev, st.st_ino)].append(rel)

    rows = []            # (category, rel, reason, old_size, new_size, note)
    old_inodes = set()
    for rel in sorted(old_files):
        st = old_files[rel]
        key = (st.st_dev, st.st_ino)
        old_inodes.add(key)
        reason = expected_reason(rel, in_tablespace)
        new_st = new_files.get(rel)

        if new_st is not None and (new_st.st_dev, new_st.st_ino) == key:
            rows.append(("LINKED", rel, reason, st.st_size, new_st.st_size,
                         "nlink=%d" % st.st_nlink))
        elif new_st is not None:
            rows.append(("COPY", rel, reason, st.st_size, new_st.st_size,
                         "old ino=%d new ino=%d" % (st.st_ino, new_st.st_ino)))
        elif new_by_inode.get(key):
            rows.append(("LINKED_ELSEWHERE", rel, reason, st.st_size, st.st_size,
                         "also at " + ", ".join(sorted(new_by_inode[key])[:3])))
        else:
            note = "nlink=%d" % st.st_nlink
            if st.st_nlink > 1:
                note += " (linked from outside the scanned trees)"
            rows.append(("MISSING", rel, reason, st.st_size, 0, note))

    extras = []
    for rel in sorted(new_files):
        st = new_files[rel]
        if rel not in old_files and (st.st_dev, st.st_ino) not in old_inodes:
            extras.append((rel, st.st_size))

    total_old = sum(st.st_size for st in old_files.values())
    total_new = sum(st.st_size for st in new_files.values())
    shared = sum(st.st_size for rel, st in new_files.items()
                 if (st.st_dev, st.st_ino) in old_inodes)

    devices = {st.st_dev for st in old_files.values()} | \
              {st.st_dev for st in new_files.values()}

    return {
        "label": label, "old_root": old_root, "new_root": new_root,
        "rows": rows, "extras": extras, "errors": old_err + new_err,
        "n_old": len(old_files), "n_new": len(new_files),
        "total_old": total_old, "total_new": total_new, "shared": shared,
        "devices": devices,
    }


def report(res, args):
    out = sys.stdout
    print("\n=== %s ===" % res["label"], file=out)
    print("  old: %s" % res["old_root"], file=out)
    print("  new: %s" % res["new_root"], file=out)

    by_cat = defaultdict(list)
    for row in res["rows"]:
        by_cat[row[0]].append(row)

    print("\n  files in old tree : %6d  (%s apparent)"
          % (res["n_old"], human(res["total_old"])), file=out)
    print("  files in new tree : %6d  (%s apparent)"
          % (res["n_new"], human(res["total_new"])), file=out)
    print("  shared via links  : %s  (single copy on disk, counted twice if you"
          " run `du` separately)" % human(res["shared"]), file=out)
    print("  only in old tree  : %s" % human(res["total_old"] - res["shared"]), file=out)
    print("  only in new tree  : %s" % human(res["total_new"] - res["shared"]), file=out)

    print("", file=out)
    for cat in ("LINKED", "LINKED_ELSEWHERE", "COPY", "MISSING"):
        rows = by_cat.get(cat, [])
        unexpected = [r for r in rows if r[2] is None and cat in ("COPY", "MISSING")]
        size = sum(r[3] for r in rows)
        line = "  %-17s %6d  %10s" % (cat, len(rows), human(size))
        if cat in ("COPY", "MISSING"):
            line += "   expected: %d, UNEXPECTED: %d" % (
                len(rows) - len(unexpected), len(unexpected))
        print(line, file=out)

    unexpected = [r for r in res["rows"]
                  if r[0] in ("COPY", "MISSING") and r[2] is None]
    if unexpected:
        print("\n  !! %d user-relation file(s) are NOT hard-linked:" % len(unexpected),
              file=out)
        shown = unexpected if args.limit == 0 else unexpected[: args.limit]
        for cat, rel, _reason, osize, nsize, note in shown:
            print("     %-8s %-60s old=%-10s new=%-10s %s"
                  % (cat, rel, human(osize), human(nsize), note), file=out)
        if len(shown) < len(unexpected):
            print("     ... and %d more (use --limit 0 or --csv)"
                  % (len(unexpected) - len(shown)), file=out)
    else:
        print("\n  OK: every user relation file in the old tree is a hard link"
              " to the same inode in the new tree.", file=out)

    if not args.quiet:
        grouped = defaultdict(lambda: [0, 0])
        for cat, _rel, reason, osize, _n, _note in res["rows"]:
            if cat in ("COPY", "MISSING") and reason is not None:
                grouped[reason][0] += 1
                grouped[reason][1] += osize
        if grouped:
            print("\n  Expected non-linked files (old side), by reason:", file=out)
            for reason, (count, size) in sorted(grouped.items(),
                                                key=lambda kv: -kv[1][1]):
                print("     %6d  %10s  %s" % (count, human(size), reason), file=out)

        if res["extras"]:
            tops = defaultdict(lambda: [0, 0])
            for rel, size in res["extras"]:
                top = rel.split(os.sep)[0]
                tops[top][0] += 1
                tops[top][1] += size
            print("\n  Files only in the new tree (new cluster's own data), by"
                  " top-level directory:", file=out)
            for top, (count, size) in sorted(tops.items(), key=lambda kv: -kv[1][1]):
                print("     %6d  %10s  %s" % (count, human(size), top), file=out)

    if args.details:
        print("\n  Full listing:", file=out)
        for cat, rel, reason, osize, nsize, note in res["rows"]:
            print("     %-17s %-60s %-10s %-10s %-40s %s"
                  % (cat, rel, human(osize), human(nsize), reason or "-", note),
                  file=out)

    if len(res["devices"]) > 1:
        print("\n  NOTE: the two trees span more than one filesystem (device ids: %s)."
              " Hard links cannot cross filesystems."
              % ", ".join(str(d) for d in sorted(res["devices"])), file=out)

    for err in res["errors"]:
        print("  ERROR: %s" % err, file=out)

    return len(unexpected)


def major_version(datadir):
    """Read the major version from datadir/PG_VERSION ('16', '18', '9.6')."""
    try:
        with open(os.path.join(datadir, "PG_VERSION")) as fh:
            return fh.read().strip()
    except OSError:
        return None


def tablespace_pairs(old_root, new_root):
    """Match PG_<major>_* version dirs reached through each pg_tblspc symlink.

    Both clusters normally point at the same tablespace location, which then
    holds one version directory per major release, so the pairing is done by
    each cluster's own major version rather than by set difference.
    """
    pairs = []
    old_tbl = os.path.join(old_root, "pg_tblspc")
    new_tbl = os.path.join(new_root, "pg_tblspc")
    old_major = major_version(old_root)
    new_major = major_version(new_root)
    if not os.path.isdir(old_tbl):
        return pairs
    if not old_major or not new_major or old_major == new_major:
        print("  WARNING: cannot determine the two major versions from PG_VERSION;"
              " skipping tablespaces", file=sys.stderr)
        return pairs

    for name in sorted(os.listdir(old_tbl)):
        old_loc = os.path.join(old_tbl, name)
        new_loc = os.path.join(new_tbl, name)
        if not (os.path.isdir(old_loc) and os.path.isdir(new_loc)):
            continue
        try:
            old_dirs = [d for d in sorted(os.listdir(old_loc))
                        if d.startswith("PG_%s_" % old_major)]
            new_dirs = [d for d in sorted(os.listdir(new_loc))
                        if d.startswith("PG_%s_" % new_major)]
        except OSError as err:
            print("  WARNING: %s" % err, file=sys.stderr)
            continue
        if len(old_dirs) == 1 and len(new_dirs) == 1:
            pairs.append((
                "tablespace %s (%s -> %s)" % (name, old_dirs[0], new_dirs[0]),
                os.path.join(old_loc, old_dirs[0]),
                os.path.join(new_loc, new_dirs[0]),
            ))
        else:
            print("  WARNING: cannot pair version directories for tablespace %s"
                  " (PG_%s_*: %s, PG_%s_*: %s)"
                  % (name, old_major, old_dirs, new_major, new_dirs),
                  file=sys.stderr)
    return pairs


def main():
    ap = argparse.ArgumentParser(
        description="Verify pg_upgrade --link hard-linked the old cluster's files.")
    ap.add_argument("old_datadir")
    ap.add_argument("new_datadir")
    ap.add_argument("--tablespaces", action="store_true",
                    help="also compare tablespace version directories")
    ap.add_argument("--details", action="store_true",
                    help="list every file, not just unexpected ones")
    ap.add_argument("--limit", type=int, default=40,
                    help="max files listed per category (0 = unlimited)")
    ap.add_argument("--csv", metavar="FILE",
                    help="write full per-file classification to FILE")
    ap.add_argument("-q", "--quiet", action="store_true")
    args = ap.parse_args()

    for path in (args.old_datadir, args.new_datadir):
        if not os.path.isdir(path):
            print("not a directory: %s" % path, file=sys.stderr)
            return 2

    trees = [("data directory", args.old_datadir, args.new_datadir, False)]
    if args.tablespaces:
        for label, old_p, new_p in tablespace_pairs(args.old_datadir,
                                                    args.new_datadir):
            trees.append((label, old_p, new_p, True))

    unexpected_total = 0
    all_rows = []
    for label, old_p, new_p, in_tbl in trees:
        res = compare_trees(label, old_p, new_p, in_tbl)
        unexpected_total += report(res, args)
        for row in res["rows"]:
            all_rows.append((label,) + row)
        for rel, size in res["extras"]:
            all_rows.append((label, "NEW_ONLY", rel, None, 0, size, ""))

    if args.csv:
        with open(args.csv, "w", newline="") as fh:
            writer = csv.writer(fh)
            writer.writerow(["tree", "category", "path", "expected_reason",
                             "old_size", "new_size", "note"])
            writer.writerows(all_rows)
        print("\nWrote %d rows to %s" % (len(all_rows), args.csv))

    print("\n%s" % ("Result: no unexpected findings." if unexpected_total == 0
                    else "Result: %d unexpected file(s) -- see above."
                         % unexpected_total))
    return 0 if unexpected_total == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
