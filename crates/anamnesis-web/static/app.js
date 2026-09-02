// Anamnesis — drag-and-drop glue (`docs/DOMAIN.md` §8: "Sortable drags,
// htmx persists"). SortableJS supplies the pointer/touch mechanics; htmx
// supplies the transport and swaps the out-of-band fragments the server
// sends back. This file owns nothing but the handoff between the two.
//
// Progressive enhancement: every card also carries a plain `<form>` (see
// `templates/_reposition_form.html`) that posts to the exact same
// `/board/reposition` endpoint. If this script — or htmx, or Sortable —
// fails to load, that form still works with JavaScript disabled entirely.
(function () {
  "use strict";

  function ready(fn) {
    if (document.readyState !== "loading") {
      fn();
    } else {
      document.addEventListener("DOMContentLoaded", fn);
    }
  }

  // Link-attachment preview: client-side only, no server fetch. Enhances
  // each `.attachment-link` into a small card (site favicon + hostname +
  // full URL) using the target site's own `/favicon.ico` -- deliberately
  // not a third-party favicon CDN, so visiting a task page never leaks its
  // attachment URLs to Google or similar. Runs independently of
  // Sortable/htmx below: without JS this is just the plain `<a>` already
  // rendered server-side (`templates/task.html`'s Attachments section).
  ready(function () {
    document.querySelectorAll(".attachment-link").forEach(function (a) {
      var href = a.getAttribute("href");
      if (!href) {
        return;
      }
      var url;
      try {
        url = new URL(href, window.location.href);
      } catch (e) {
        return;
      }
      if (url.protocol !== "http:" && url.protocol !== "https:") {
        return;
      }

      var favicon = document.createElement("img");
      favicon.className = "attachment-favicon";
      favicon.alt = "";
      favicon.src = url.origin + "/favicon.ico";
      favicon.onerror = function () {
        favicon.remove();
      };

      var host = document.createElement("span");
      host.className = "attachment-host";
      host.textContent = url.hostname;

      var full = document.createElement("span");
      full.className = "attachment-url";
      full.textContent = url.href;

      var text = document.createElement("span");
      text.className = "attachment-text";
      text.appendChild(host);
      text.appendChild(full);

      var card = document.createElement("span");
      card.className = "attachment-preview";
      card.appendChild(favicon);
      card.appendChild(text);

      a.textContent = "";
      a.appendChild(card);
      a.classList.add("attachment-link-enhanced");
    });
  });

  ready(function () {
    if (typeof window.Sortable === "undefined" || typeof window.htmx === "undefined") {
      return;
    }

    var csrfMeta = document.querySelector('meta[name="csrf-token"]');
    var csrfToken = csrfMeta ? csrfMeta.content : "";

    function initBoardSortable() {
      document.querySelectorAll(".card-list[data-column-id]").forEach(function (list) {
        if (window.Sortable.get(list)) {
          return;
        }
        new window.Sortable(list, {
          group: "anamnesis-board-cards",
          animation: 150,
          ghostClass: "card-drag-ghost",
          // A touch-friendly delay so an ordinary tap/scroll on mobile is
          // not mistaken for a drag start.
          delay: 120,
          delayOnTouchOnly: true,
          onEnd: function (evt) {
            var card = evt.item;
            var kind = card.getAttribute("data-item-kind");
            var id = card.getAttribute("data-item-id");
            var columnId = evt.to.getAttribute("data-column-id");
            var position = evt.newIndex;
            if (!kind || !id || !columnId || position === null || position === undefined) {
              return;
            }
            window.htmx.ajax("POST", "/board/reposition", {
              source: card,
              target: "body",
              swap: "none",
              values: {
                csrf_token: csrfToken,
                item_kind: kind,
                item_id: id,
                column_id: columnId,
                position: String(position),
              },
            });
          },
        });
      });
    }

    // The project page's two flat lists ("On the board" / "Below the
    // horizon", `templates/_project_task_list.html`) — a separate Sortable
    // `group` from the board's cards above, so dragging never crosses
    // between the two page types. There is no position to persist within a
    // list (the flat list carries no ordering field), so an intra-list drag
    // is a no-op; only crossing from one list to the other raises or drops
    // the task.
    function initProjectSortable() {
      document.querySelectorAll(".task-list.drag-list[data-role]").forEach(function (list) {
        if (window.Sortable.get(list)) {
          return;
        }
        new window.Sortable(list, {
          group: "anamnesis-project-tasks",
          animation: 150,
          ghostClass: "task-drag-ghost",
          delay: 120,
          delayOnTouchOnly: true,
          onEnd: function (evt) {
            if (evt.from === evt.to) {
              return;
            }
            var item = evt.item;
            var taskId = item.getAttribute("data-item-id");
            var projectId = evt.to.getAttribute("data-project-id");
            var destRole = evt.to.getAttribute("data-role");
            if (!taskId || !projectId || !destRole) {
              return;
            }
            var action = destRole === "on_board" ? "raise" : "drop";
            window.htmx.ajax("POST", "/projects/" + projectId + "/tasks/" + taskId + "/" + action, {
              source: item,
              target: "body",
              swap: "none",
              values: { csrf_token: csrfToken },
            });
          },
        });
      });
    }

    initBoardSortable();
    initProjectSortable();

    // Both raise/drop and reposition persist by having the server swap a
    // fresh `<ul>` in out-of-band (`hx-swap-oob="true"`, `_column.html` and
    // `_project_task_list.html`) rather than patching the existing one —
    // the new node replaces the old Sortable-instrumented one wholesale, so
    // its Sortable instance (and listeners) are gone. Re-running both init
    // functions after every htmx swap re-attaches Sortable to whatever list
    // nodes are now in the DOM; the `Sortable.get` guard above makes this a
    // no-op for any list an oob swap didn't touch.
    document.body.addEventListener("htmx:afterSwap", function () {
      initBoardSortable();
      initProjectSortable();
    });
    document.body.addEventListener("htmx:oobAfterSwap", function () {
      initBoardSortable();
      initProjectSortable();
    });
  });
})();
