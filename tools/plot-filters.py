# tools/plot_filters.py
import re
import matplotlib.pyplot as plt

pattern = re.compile(r"PLOT raw=(\d+) ema=(\d+) median=(\d+)")

raw, ema, median = [], [], []
with open("filter_log.txt") as f:
    for line in f:
        m = pattern.search(line)
        if m:
            raw.append(int(m.group(1)))
            ema.append(int(m.group(2)))
            median.append(int(m.group(3)))

plt.plot(raw, label="raw", alpha=0.4)
plt.plot(ema, label="ema")
plt.plot(median, label="median")
plt.xlabel("sample #")
plt.ylabel("ADC counts")
plt.legend()
plt.title("Filter comparison")
plt.savefig("filter_comparison_higher_res.png", dpi=600)
plt.show()