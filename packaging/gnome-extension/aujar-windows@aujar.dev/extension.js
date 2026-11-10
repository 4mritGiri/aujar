// Aujar Windows: a tiny D-Bus service for the Aujar launcher's window switcher.
//
//   bus name     dev.aujar.Windows
//   object path  /dev/aujar/Windows
//   List()       -> s    JSON array: [{id, title, app, app_id, wm_class, workspace, focused}]
//   Activate(u)  -> b    raise and focus the window with that id
//
//   object path  /dev/aujar/Pointer       (interface dev.aujar.Pointer, same bus name)
//   SetHidden(b) -> b    hide (true) or show (false) the mouse pointer
//
// GNOME gives other applications no way to see windows, so this runs inside the shell.
// It only reads window metadata and focuses windows; it cannot close, move or type into them.
// The pointer method is for Aujar Mouse Borders: while you control another computer, the
// pointer of this one would otherwise sit visible at the screen edge. It cannot stay hidden:
// the request has to be renewed within a minute, and disabling the extension shows it again.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const IFACE = `
<node>
  <interface name="dev.aujar.Windows">
    <method name="List">
      <arg type="s" direction="out" name="windows"/>
    </method>
    <method name="Activate">
      <arg type="u" direction="in" name="id"/>
      <arg type="b" direction="out" name="ok"/>
    </method>
  </interface>
</node>`;

const POINTER_IFACE = `
<node>
  <interface name="dev.aujar.Pointer">
    <method name="SetHidden">
      <arg type="b" direction="in" name="hidden"/>
      <arg type="b" direction="out" name="ok"/>
    </method>
  </interface>
</node>`;

// A hidden pointer comes back by itself after this long unless the request is renewed.
const HIDE_SECONDS = 60;

export default class AujarWindows extends Extension {
    enable() {
        this._service = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._service.export(Gio.DBus.session, '/dev/aujar/Windows');
        this._pointer = Gio.DBusExportedObject.wrapJSObject(POINTER_IFACE, this);
        this._pointer.export(Gio.DBus.session, '/dev/aujar/Pointer');
        this._ownerId = Gio.bus_own_name_on_connection(
            Gio.DBus.session, 'dev.aujar.Windows',
            Gio.BusNameOwnerFlags.REPLACE, null, null);
    }

    disable() {
        this._showPointer();

        if (this._pointer) {
            this._pointer.unexport();
            this._pointer = null;
        }
        if (this._ownerId) {
            Gio.bus_unown_name(this._ownerId);
            this._ownerId = 0;
        }
        if (this._service) {
            this._service.unexport();
            this._service = null;
        }
    }

    _cursorTracker() {
        return global.backend.get_cursor_tracker?.() ??
            Meta.CursorTracker.get_for_display(global.display);
    }

    _showPointer() {
        if (this._hideTimer) {
            GLib.source_remove(this._hideTimer);
            this._hideTimer = 0;
        }

        try {
            this._cursorTracker().set_pointer_visible(true);
        } catch (error) {
            logError(error, 'Aujar Windows: cannot show the pointer');
        }
    }

    SetHidden(hidden) {
        if (!hidden) {
            this._showPointer();
            return true;
        }

        try {
            this._cursorTracker().set_pointer_visible(false);
        } catch (error) {
            logError(error, 'Aujar Windows: cannot hide the pointer');
            return false;
        }

        // Renewing the request starts the count again.
        if (this._hideTimer)
            GLib.source_remove(this._hideTimer);

        this._hideTimer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, HIDE_SECONDS, () => {
            this._hideTimer = 0;
            this._showPointer();
            return GLib.SOURCE_REMOVE;
        });

        return true;
    }

    _windows() {
        // Most recently used first, so an empty filter shows what you were just doing.
        return global.display
            .get_tab_list(Meta.TabList.NORMAL, null)
            .filter(window => !window.is_skip_taskbar());
    }

    List() {
        const tracker = Shell.WindowTracker.get_default();
        const focused = global.display.focus_window;

        const list = this._windows().map(window => {
            const app = tracker.get_window_app(window);
            const workspace = window.get_workspace();

            return {
                id: window.get_id(),
                title: window.get_title() ?? '',
                app: app?.get_name() ?? window.get_wm_class() ?? '',
                app_id: app?.get_id() ?? null,
                wm_class: window.get_wm_class() ?? null,
                workspace: workspace ? workspace.index() : -1,
                focused: window === focused,
            };
        });

        return JSON.stringify(list);
    }

    Activate(id) {
        const window = this._windows().find(candidate => candidate.get_id() === id);

        if (!window)
            return false;

        Main.activateWindow(window);
        return true;
    }
}
