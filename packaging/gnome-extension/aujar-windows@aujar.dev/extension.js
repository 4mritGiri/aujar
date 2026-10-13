// Aujar Windows: a tiny D-Bus service for the Aujar launcher's window switcher.
//
//   bus name     dev.aujar.Windows
//   object path  /dev/aujar/Windows
//   List()       -> s    JSON array: [{id, title, app, app_id, wm_class, workspace, focused}]
//   Activate(u)  -> b    raise and focus the window with that id
//
// GNOME gives other applications no way to see windows, so this runs inside the shell.
// It only reads window metadata and focuses windows; it cannot close, move or type into them.

import Gio from 'gi://Gio';
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

export default class AujarWindows extends Extension {
    enable() {
        this._service = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._service.export(Gio.DBus.session, '/dev/aujar/Windows');
        this._ownerId = Gio.bus_own_name_on_connection(
            Gio.DBus.session, 'dev.aujar.Windows',
            Gio.BusNameOwnerFlags.REPLACE, null, null);
    }

    disable() {
        if (this._ownerId) {
            Gio.bus_unown_name(this._ownerId);
            this._ownerId = 0;
        }
        if (this._service) {
            this._service.unexport();
            this._service = null;
        }
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
