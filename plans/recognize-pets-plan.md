# Pet recognition (plan, not started)

Status: estimated only; deliberately postponed.

## Today

There is no pet recognition. SCRFD (detection) and ArcFace (embedding) in `face.rs` only
work on human faces. A pet is findable only through the free-text description the vision
model writes ("dog", "cat").

## Goal

Tell the same pet apart across photos, the way Faces does for people: grouping, naming,
merge, ignore, re-analyze and a `pet:<id>` search filter.

## Approach (MVP)

1. **Detection**: an ONNX YOLO model (COCO has cat and dog) finds the animal.
   Reuses the ONNX runtime, model download and thread pool of `face.rs`. Effort: low to medium.
2. **Identity** (the hard part): ArcFace does not apply. Embed the animal crop with
   DINOv2 or CLIP (generic, reasonable). A dedicated pet re-identification model would be
   better but there are few ready ONNX options. Effort: medium. Expect lower precision than
   for humans: similar coats or the same breed will merge, so the strictness setting and
   "Re-analyze faces" matter even more here.
3. **Data and UI**: reuse clustering, merge, ignore, the refine modal and the context menus.
   Cheapest shape: append a migration `ALTER TABLE people ADD COLUMN kind TEXT NOT NULL
   DEFAULT 'person'` (never edit shipped migrations) and a "Pets" tab in the Faces screen.
   Pets need their own quality gates (blur, minimum size, confidence).
4. **Extras**: on/off setting, `pet:<id>` filter, embedding storage, 100-300 MB of extra
   model downloads, tests, CI on Linux, macOS and Windows.

## Estimate

- MVP (YOLO + DINOv2/CLIP + reused clustering + Pets tab): about 3 to 5 days.
- Polished (calibrated thresholds on real photos, UX, web filter): 1 to 2 weeks, mostly
  model testing and calibration.

## Cheap alternative

Have the Ollama vision model (qwen2.5vl) tag "has dog/cat" during classification and filter
by that tag. It does not tell *which* pet, but costs almost nothing.

## Open questions

- Which embedding model gives acceptable separation between similar pets on the user's photos?
- Separate `pets` table versus `kind` column on `people`?
- Should pets share the strictness setting or get their own?
