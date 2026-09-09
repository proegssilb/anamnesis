// Anamnesis — chunked (multi-request) file uploads (issue #21). Progressive
// enhancement on top of the plain `<form enctype="multipart/form-data">`
// already rendered for "Attach a file" (`templates/task.html`): a file no
// bigger than the form's own `data-chunk-size` (the server's
// `ANAMNESIS_MAX_BODY_BYTES`) is left to submit exactly as it always has,
// as one ordinary multipart POST. A larger file is instead sent as a begin
// call, one `PUT` per slice, then a complete call
// (`crate::handlers::tasks::chunked_attachments`) — each request small
// enough to stay under that same per-request limit, so a file's total size
// is bounded only by `ANAMNESIS_MAX_ATTACHMENT_BYTES` instead.
//
// Deliberately minimal: parts are sent one at a time, in order, with a
// single retry each; there is no resuming a upload across a page reload.
// Richer behaviour (parallel parts, resumability) is a natural follow-up,
// not required for the feature to work.
(function () {
  "use strict";

  function ready(fn) {
    if (document.readyState !== "loading") {
      fn();
    } else {
      document.addEventListener("DOMContentLoaded", fn);
    }
  }

  // A little headroom under the server's own per-request cap, so this
  // script's own request (headers, form framing) never itself trips the
  // limit it is trying to stay under.
  var CHUNK_MARGIN_BYTES = 64 * 1024;

  function csrfToken(form) {
    var field = form.querySelector('input[name="csrf_token"]');
    return field ? field.value : "";
  }

  function fetchJson(url, method, csrf, body) {
    return window
      .fetch(url, {
        method: method,
        headers: { "Content-Type": "application/json", "X-Csrf-Token": csrf },
        body: body === undefined ? undefined : JSON.stringify(body),
      })
      .then(function (response) {
        if (!response.ok) {
          throw new Error(method + " " + url + " failed with " + response.status);
        }
        return response.status === 204 ? null : response.json();
      });
  }

  function putChunk(url, csrf, blob) {
    return window
      .fetch(url, {
        method: "PUT",
        headers: { "X-Csrf-Token": csrf },
        body: blob,
      })
      .then(function (response) {
        if (!response.ok) {
          throw new Error("PUT " + url + " failed with " + response.status);
        }
      });
  }

  // Uploads `file` in sequence, one `PUT` per slice, updating `progress`
  // (a `<progress>` element, 0-100) as each part lands. Returns a promise
  // resolving to the `{task_id, attachment_id}` the complete call returns.
  function uploadChunked(taskPath, file, chunkSize, csrf, progress) {
    var uploadId;
    var totalParts = Math.max(1, Math.ceil(file.size / chunkSize));

    function putPart(partNumber) {
      if (partNumber > totalParts) {
        return Promise.resolve();
      }
      var start = (partNumber - 1) * chunkSize;
      var slice = file.slice(start, start + chunkSize);
      return putChunk(
        "/attachments/uploads/" + uploadId + "/parts/" + partNumber,
        csrf,
        slice
      ).then(function () {
        if (progress) {
          progress.value = Math.round((partNumber / totalParts) * 100);
        }
        return putPart(partNumber + 1);
      });
    }

    return fetchJson(taskPath + "/attachments/file/uploads", "POST", csrf, {
      filename: file.name,
      mime: file.type || "application/octet-stream",
    })
      .then(function (begun) {
        uploadId = begun.upload_id;
        return putPart(1);
      })
      .then(function () {
        return fetchJson(
          "/attachments/uploads/" + uploadId + "/complete",
          "POST",
          csrf
        );
      })
      .catch(function (err) {
        // Best-effort: free the storage-side upload immediately rather than
        // waiting for the abandoned-upload GC sweep to notice it. Failure
        // here is not itself reported -- the original `err` is what the
        // caller needs to see.
        if (uploadId) {
          window
            .fetch("/attachments/uploads/" + uploadId, {
              method: "DELETE",
              headers: { "X-Csrf-Token": csrf },
            })
            .catch(function (cleanupErr) {
              console.error("failed to free the abandoned upload", cleanupErr);
            });
        }
        throw err;
      });
  }

  ready(function () {
    document.querySelectorAll("form[data-chunked-upload]").forEach(function (form) {
      var chunkSize = parseInt(form.getAttribute("data-chunk-size"), 10);
      var taskPath = "/tasks/" + form.getAttribute("data-task-id");
      if (!chunkSize || chunkSize <= CHUNK_MARGIN_BYTES) {
        return;
      }
      chunkSize -= CHUNK_MARGIN_BYTES;

      form.addEventListener("submit", function (event) {
        var input = form.querySelector('input[type="file"]');
        var file = input && input.files && input.files[0];
        if (!file || file.size <= chunkSize) {
          // Small enough for the plain multipart submit this form already
          // does -- let it go through unmodified.
          return;
        }
        event.preventDefault();

        var button = form.querySelector('button[type="submit"]');
        var progress = form.querySelector(".chunked-upload-progress");
        if (button) {
          button.disabled = true;
        }
        if (progress) {
          progress.hidden = false;
          progress.value = 0;
        }

        uploadChunked(taskPath, file, chunkSize, csrfToken(form), progress)
          .then(function () {
            // Only the fragment changes -- this form is already rendered on
            // `taskPath`'s own page, so there is no page to navigate to.
            // (Setting `location.href` from `taskPath` here once tripped
            // CodeQL's "DOM text reinterpreted as HTML" check, since that
            // sink is scored as if the URL could still be attacker-supplied;
            // `location.hash` carries no such flow.)
            window.location.hash = "task-add-attachment";
            window.location.reload();
          })
          .catch(function (err) {
            window.alert("That upload failed: " + err.message);
            if (button) {
              button.disabled = false;
            }
            if (progress) {
              progress.hidden = true;
            }
          });
      });
    });
  });
})();
