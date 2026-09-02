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

    // Shared by all three init functions below: guard against re-attaching
    // Sortable to a list that already has an instance, then wire up the
    // options every list shares (only `group`/`ghostClass`/`onEnd` vary per
    // list kind).
    function initSortable(selector, group, ghostClass, onEnd) {
      document.querySelectorAll(selector).forEach(function (list) {
        if (window.Sortable.get(list)) {
          return;
        }
        new window.Sortable(list, {
          group: group,
          animation: 150,
          ghostClass: ghostClass,
          // A touch-friendly delay so an ordinary tap/scroll on mobile is
          // not mistaken for a drag start.
          delay: 120,
          delayOnTouchOnly: true,
          onEnd: onEnd,
        });
      });
    }

    // The task board's columns. Unlike the two lists below, there is a
    // position to persist *within* a column (`data-item-kind`/-id order), so
    // this is the one init function with no `evt.from === evt.to` guard --
    // an intra-column drag is a real reposition, not a no-op.
    function initBoardSortable() {
      initSortable(".card-list[data-column-id]", "anamnesis-board-cards", "card-drag-ghost", function (evt) {
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
      initSortable(".task-list.drag-list[data-role]", "anamnesis-project-tasks", "task-drag-ghost", function (evt) {
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
      });
    }

    // The area page's Pending/Active/Complete project lanes
    // (`templates/_area_project_list.html`) -- a third Sortable `group`,
    // separate from the board's cards and the project page's task lists
    // above, so dragging never crosses between page types. Like the project
    // task lists, there is no position to persist within a lane; dragging a
    // project card into a different lane fires the same
    // `/projects/{id}/status` transition the select+button "Move" form
    // already posts to. Only rendered as a drag target for callers who can
    // manage the area (`can_manage` in the template) -- a plain Member sees
    // static, non-interactive cards.
    function initAreaSortable() {
      initSortable(
        ".card-list.drag-list[data-role][data-area-id]",
        "anamnesis-area-projects",
        "card-drag-ghost",
        function (evt) {
          if (evt.from === evt.to) {
            return;
          }
          var item = evt.item;
          var projectId = item.getAttribute("data-item-id");
          var destRole = evt.to.getAttribute("data-role");
          if (!projectId || !destRole) {
            return;
          }
          window.htmx.ajax("POST", "/projects/" + projectId + "/status", {
            source: item,
            target: "body",
            swap: "none",
            values: { csrf_token: csrfToken, status: destRole },
          });
        }
      );
    }

    function initAllSortables() {
      initBoardSortable();
      initProjectSortable();
      initAreaSortable();
    }

    initAllSortables();

    // Both raise/drop and reposition persist by having the server swap a
    // fresh `<ul>` in out-of-band (`hx-swap-oob="true"`, `_column.html` and
    // `_project_task_list.html`) rather than patching the existing one —
    // the new node replaces the old Sortable-instrumented one wholesale, so
    // its Sortable instance (and listeners) are gone. Re-running all three
    // init functions after every htmx swap re-attaches Sortable to whatever
    // list nodes are now in the DOM; the `Sortable.get` guard in
    // `initSortable` above makes this a no-op for any list an oob swap
    // didn't touch.
    document.body.addEventListener("htmx:afterSwap", initAllSortables);
    document.body.addEventListener("htmx:oobAfterSwap", initAllSortables);
  });
})();
