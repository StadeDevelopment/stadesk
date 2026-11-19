#!/usr/bin/env python3
import json
import os
import subprocess
import sys
import time

from gi.repository import Gio, GLib

BUS_NAME = "org.freedesktop.impl.portal.desktop.stadesktest"
OBJECT_PATH = "/org/freedesktop/portal/desktop"
WIDTH = 640
HEIGHT = 480
LOG_PATH = os.environ.get("MOCK_PORTAL_LOG", "/tmp/mock-portal.jsonl")
REMOTE_DESKTOP = os.environ.get("MOCK_REMOTE_DESKTOP", "1") == "1"

SCREEN_CAST_XML = """
<node>
  <interface name="org.freedesktop.impl.portal.ScreenCast">
    <method name="CreateSession">
      <arg type="o" name="handle" direction="in"/>
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="s" name="app_id" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/>
      <arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="SelectSources">
      <arg type="o" name="handle" direction="in"/>
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="s" name="app_id" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/>
      <arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="Start">
      <arg type="o" name="handle" direction="in"/>
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/>
      <arg type="a{sv}" name="results" direction="out"/>
    </method>
    <property name="AvailableSourceTypes" type="u" access="read"/>
    <property name="AvailableCursorModes" type="u" access="read"/>
    <property name="version" type="u" access="read"/>
  </interface>
</node>
"""

REMOTE_DESKTOP_XML = """
<node>
  <interface name="org.freedesktop.impl.portal.RemoteDesktop">
    <method name="CreateSession">
      <arg type="o" name="handle" direction="in"/>
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="s" name="app_id" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/>
      <arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="SelectDevices">
      <arg type="o" name="handle" direction="in"/>
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="s" name="app_id" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/>
      <arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="Start">
      <arg type="o" name="handle" direction="in"/>
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/>
      <arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="NotifyPointerMotion">
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="d" name="dx" direction="in"/>
      <arg type="d" name="dy" direction="in"/>
    </method>
    <method name="NotifyPointerMotionAbsolute">
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="stream" direction="in"/>
      <arg type="d" name="x" direction="in"/>
      <arg type="d" name="y" direction="in"/>
    </method>
    <method name="NotifyPointerButton">
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="i" name="button" direction="in"/>
      <arg type="u" name="state" direction="in"/>
    </method>
    <method name="NotifyPointerAxis">
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="d" name="dx" direction="in"/>
      <arg type="d" name="dy" direction="in"/>
    </method>
    <method name="NotifyPointerAxisDiscrete">
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="axis" direction="in"/>
      <arg type="i" name="steps" direction="in"/>
    </method>
    <method name="NotifyKeyboardKeycode">
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="i" name="keycode" direction="in"/>
      <arg type="u" name="state" direction="in"/>
    </method>
    <method name="NotifyKeyboardKeysym">
      <arg type="o" name="session_handle" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="i" name="keysym" direction="in"/>
      <arg type="u" name="state" direction="in"/>
    </method>
    <property name="AvailableDeviceTypes" type="u" access="read"/>
    <property name="version" type="u" access="read"/>
  </interface>
</node>
"""

SESSION_XML = """
<node>
  <interface name="org.freedesktop.impl.portal.Session">
    <method name="Close"/>
    <signal name="Closed"/>
    <property name="version" type="u" access="read"/>
  </interface>
</node>
"""

source_process = None
node_id = None
launches = 0
sessions = {}


def log(entry):
    with open(LOG_PATH, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry) + "\n")


def start_source():
    global source_process, node_id, launches
    if node_id is not None and source_process is not None and source_process.poll() is None:
        return node_id
    node_id = None
    launches += 1
    name = f"stadesk-mock-source-{launches}"
    source_process = subprocess.Popen(
        [
            "gst-launch-1.0",
            "-q",
            "videotestsrc",
            "pattern=smpte",
            "is-live=true",
            "!",
            f"video/x-raw,format=BGRx,width={WIDTH},height={HEIGHT},framerate=30/1",
            "!",
            "pipewiresink",
            "mode=provide",
            f"stream-properties=props,node.name={name},media.class=Video/Source",
        ]
    )
    for _ in range(100):
        dump = json.loads(subprocess.run(["pw-dump"], capture_output=True, text=True).stdout or "[]")
        for item in dump:
            props = (item.get("info") or {}).get("props") or {}
            if item.get("type") == "PipeWire:Interface:Node" and props.get("node.name") == name:
                node_id = item["id"]
                log({"event": "source", "node": node_id})
                return node_id
        time.sleep(0.1)
    raise RuntimeError("the mock video source did not appear in PipeWire")


def stream_list():
    stream = start_source()
    return GLib.Variant(
        "a(ua{sv})",
        [(stream, {"size": GLib.Variant("(ii)", (WIDTH, HEIGHT)), "source_type": GLib.Variant("u", 1)})],
    )


def export_session(connection, path):
    info = Gio.DBusNodeInfo.new_for_xml(SESSION_XML)

    def call(connection, sender, object_path, interface, method, parameters, invocation):
        if method == "Close":
            log({"event": "session-closed"})
            registration = sessions.pop(object_path, None)
            if registration is not None:
                connection.unregister_object(registration)
            invocation.return_value(None)

    def get_property(connection, sender, object_path, interface, name):
        return GLib.Variant("u", 1)

    sessions[path] = connection.register_object(path, info.interfaces[0], call, get_property, None)


def ok(invocation, results=None):
    invocation.return_value(GLib.Variant("(ua{sv})", (0, results or {})))


def screen_cast_call(connection, sender, object_path, interface, method, parameters, invocation):
    arguments = parameters.unpack()
    if method == "CreateSession":
        export_session(connection, arguments[1])
        ok(invocation, {"session_id": GLib.Variant("s", "mock")})
    elif method == "SelectSources":
        log({"event": "select-sources", "options": {key: str(value) for key, value in arguments[3].items()}})
        ok(invocation)
    elif method == "Start":
        ok(invocation, {"streams": stream_list()})


def screen_cast_property(connection, sender, object_path, interface, name):
    return {
        "AvailableSourceTypes": GLib.Variant("u", 1),
        "AvailableCursorModes": GLib.Variant("u", 1 | 2),
        "version": GLib.Variant("u", 5),
    }[name]


def remote_desktop_call(connection, sender, object_path, interface, method, parameters, invocation):
    arguments = parameters.unpack()
    if method == "CreateSession":
        export_session(connection, arguments[1])
        ok(invocation, {"session_id": GLib.Variant("s", "mock")})
    elif method == "SelectDevices":
        ok(invocation)
    elif method == "Start":
        ok(
            invocation,
            {
                "devices": GLib.Variant("u", 1 | 2),
                "streams": stream_list(),
                "clipboard_enabled": GLib.Variant("b", False),
            },
        )
    else:
        log({"event": method, "arguments": list(arguments[2:])})
        invocation.return_value(None)


def remote_desktop_property(connection, sender, object_path, interface, name):
    return {
        "AvailableDeviceTypes": GLib.Variant("u", 1 | 2),
        "version": GLib.Variant("u", 2),
    }[name]


def on_bus_acquired(connection, name):
    screen_cast = Gio.DBusNodeInfo.new_for_xml(SCREEN_CAST_XML)
    connection.register_object(
        OBJECT_PATH, screen_cast.interfaces[0], screen_cast_call, screen_cast_property, None
    )
    if REMOTE_DESKTOP:
        remote_desktop = Gio.DBusNodeInfo.new_for_xml(REMOTE_DESKTOP_XML)
        connection.register_object(
            OBJECT_PATH, remote_desktop.interfaces[0], remote_desktop_call, remote_desktop_property, None
        )


def main():
    loop = GLib.MainLoop()
    Gio.bus_own_name(
        Gio.BusType.SESSION,
        BUS_NAME,
        Gio.BusNameOwnerFlags.NONE,
        on_bus_acquired,
        lambda connection, name: log({"event": "ready"}),
        lambda connection, name: loop.quit(),
    )
    try:
        loop.run()
    finally:
        if source_process is not None:
            source_process.terminate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
