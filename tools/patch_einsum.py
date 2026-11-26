#!/usr/bin/env python3
"""Rewrites the Einsum equations of DocAligner's corner model so tract can load it.

The 15 weighted-sum nodes of the BiFPN neck use the equation "i,i...->..." with a rank 5 second input; tract rejects the
ellipsis. Each becomes "i,ibcde->bcde". Only protobuf lengths change, nothing else in the file, so the output is the
same bytes on every run. Usage: patch_einsum.py in.onnx out.onnx
"""
import sys

OLD = b"i,i...->..."
NEW = b"i,ibcde->bcde"

# ModelProto.graph, GraphProto.node, NodeProto.attribute
GRAPH, NODE, ATTRIBUTE = 7, 1, 5


def read_varint(b, i):
    value = shift = 0
    while True:
        c = b[i]
        i += 1
        value |= (c & 0x7F) << shift
        if c < 0x80:
            return value, i
        shift += 7


def varint(n):
    out = bytearray()
    while True:
        low = n & 0x7F
        n >>= 7
        if n:
            out.append(low | 0x80)
        else:
            out.append(low)
            return bytes(out)


def split(b):
    """The message as a list of (field, wire type, value); length-delimited values stay bytes."""
    i = 0
    fields = []
    while i < len(b):
        key, i = read_varint(b, i)
        field, wire = key >> 3, key & 7
        if wire == 0:
            start = i
            _, i = read_varint(b, i)
            fields.append((field, wire, b[start:i]))
        elif wire == 2:
            n, i = read_varint(b, i)
            fields.append((field, wire, b[i:i + n]))
            i += n
        elif wire == 1:
            fields.append((field, wire, b[i:i + 8]))
            i += 8
        elif wire == 5:
            fields.append((field, wire, b[i:i + 4]))
            i += 4
        else:
            raise ValueError(f"wire type {wire}")
    return fields


def join(fields):
    out = bytearray()
    for field, wire, value in fields:
        out += varint(field << 3 | wire)
        if wire == 2:
            out += varint(len(value))
        out += value
    return bytes(out)


def patch_attribute(b):
    fields = split(b)
    equation = any(f == 1 and v == b"equation" for f, _, v in fields)
    if not equation:
        return b, 0
    count = 0
    out = []
    for f, w, v in fields:
        if f == 4 and v == OLD:
            v = NEW
            count += 1
        out.append((f, w, v))
    return join(out), count


def patch_node(b):
    fields = split(b)
    if not any(f == 4 and v == b"Einsum" for f, _, v in fields):
        return b, 0
    count = 0
    out = []
    for f, w, v in fields:
        if f == ATTRIBUTE:
            v, n = patch_attribute(v)
            count += n
        out.append((f, w, v))
    return join(out), count


def patch_graph(b):
    count = 0
    out = []
    for f, w, v in split(b):
        if f == NODE:
            v, n = patch_node(v)
            count += n
        out.append((f, w, v))
    return join(out), count


def patch_model(b):
    count = 0
    out = []
    for f, w, v in split(b):
        if f == GRAPH:
            v, n = patch_graph(v)
            count += n
        out.append((f, w, v))
    return join(out), count


if __name__ == "__main__":
    source, target = sys.argv[1:3]
    patched, changed = patch_model(open(source, "rb").read())
    if changed != 15:
        sys.exit(f"expected 15 Einsum equations, rewrote {changed}")
    open(target, "wb").write(patched)
    print(f"rewrote {changed} equations")
