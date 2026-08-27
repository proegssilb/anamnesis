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

  ready(function () {
    if (typeof window.Sortable === "undefined" || typeof window.htmx === "undefined") {
      return;
    }

    var csrfMeta = document.querySelector('meta[name="csrf-token"]');
    var csrfToken = csrfMeta ? csrfMeta.content : "";

    document.querySelectorAll(".card-list[data-column-id]").forEach(function (list) {
      new window.Sortable(list, {
        group: "anamnesis-board-cards",
        animation: 150,
        ghostClass: "card-drag-ghost",
        // A touch-friendly delay so an ordinary tap/scroll on mobile is not
        // mistaken for a drag start.
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
  });
})();
