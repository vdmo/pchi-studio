"""Send in- and out-of-bounds positions straight to a running Conductor and print how many governance
receipts exist after each. Start the Conductor first with --rules rgbmocap-stage.only-pchi."""
import json, socket, sys, time, urllib.request
sys.path.insert(0, r"C:\pir-main\tools\rgbmocap-pchi-bridge")
from rgbmocap_pchi_bridge import pchi_message, control_message
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
def send(m): s.sendto(json.dumps(m).encode(), ("127.0.0.1", 8888)); time.sleep(0.15)
def receipts():
    r = json.load(urllib.request.urlopen("http://127.0.0.1:3000/governance/export"))
    return r["count"]
print("start receipts=", receipts())
def step(label, m): send(m); print(f"{label:<44} receipts={receipts()}")
step("in bounds (0.5, 0, 1.2)", pchi_message((0.5,0,1.2),(0,0,0),[]))
step("OUT: x=3.0", pchi_message((3.0,0,1.2),(0,0,0),[]))
step("unrelated control_parameter (position stale)", control_message("rgbmocap.energy", 0.2))
step("unrelated control_parameter again", control_message("rgbmocap.energy", 0.3))
step("OUT: z=-0.5", pchi_message((0,0,-0.5),(0,0,0),[]))
step("back in bounds (0, 0, 1.0)", pchi_message((0,0,1.0),(0,0,0),[]))
step("unrelated control_parameter after recovery", control_message("rgbmocap.energy", 0.4))
