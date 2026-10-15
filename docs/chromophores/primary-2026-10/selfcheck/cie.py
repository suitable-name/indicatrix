"""CIELAB from a transmittance spectrum, per the request's method:
380-780 nm at 5 nm, CIE 1931 2 deg, D65 or A (2856 K), Lab vs the illuminant's own white."""
import sys
sys.path.insert(0, "/tmp/claude-0/-home-claude/c93b7df0-eaf4-5b3e-931d-e62b8c066c41/scratchpad/color")
import numpy as np
import color

WL = np.arange(380, 781, 5, dtype=float)
_CMF = color.MSDS_CMFS["CIE 1931 2 Degree Standard Observer"]
CMF = np.array([_CMF[w] for w in WL])
ILL = {k: np.array([color.SDS_ILLUMINANTS[k][w] for w in WL]) for k in ("D65", "A")}

def lab(T, ill="D65"):
    S = ILL[ill]
    k = 100.0 / np.sum(S * CMF[:, 1])
    XYZ = k * np.sum((S * T)[:, None] * CMF, axis=0)
    W = k * np.sum(S[:, None] * CMF, axis=0)
    return color.XYZ_to_Lab(XYZ / 100, color.XYZ_to_xy(W / 100))

def de00(a, b):
    return float(color.delta_E(a, b, method="CIE 2000"))

def interp(wl, y):
    o = np.argsort(wl)
    return np.interp(WL, np.asarray(wl)[o], np.asarray(y)[o])

def load_xy(path, skip=2, sep=None):
    xs, ys = [], []
    for i, line in enumerate(open(path, encoding="latin-1")):
        if i < skip or not line.strip():
            continue
        p = line.replace(",", " ").split() if sep is None else line.split(sep)
        try:
            xs.append(float(p[0])); ys.append(float(p[1]))
        except (ValueError, IndexError):
            pass
    return np.array(xs), np.array(ys)
