import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { X } from "@phosphor-icons/react";
import type { InstalledSkill } from "../../domain/types";

export function SkillSelectorDialog({
  skills,
  selectedSkills,
  loading,
  error,
  onRetry,
  onCancel,
  onApply,
}: {
  skills: InstalledSkill[] | null;
  selectedSkills: string[] | undefined;
  loading: boolean;
  error: string;
  onRetry: () => void;
  onCancel: () => void;
  onApply: (skills: string[] | undefined) => void;
}) {
  const search = useRef<HTMLInputElement>(null);
  const selectAll = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [selectedIds, setSelectedIds] = useState<string[]>(
    selectedSkills ?? [],
  );
  const [selectAllMode, setSelectAllMode] = useState(
    selectedSkills === undefined,
  );

  useLayoutEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    search.current?.focus();
    return () => {
      if (opener?.isConnected) opener.focus();
    };
  }, []);

  useEffect(() => {
    if (skills === null || loading || error) return;
    const all = selectedSkills === undefined;
    setSelectAllMode(all);
    setSelectedIds(
      selectedSkills === undefined
        ? skills.map((skill) => skill.id)
        : selectedSkills,
    );
  }, [skills, selectedSkills, loading, error]);

  const selected = new Set(selectedIds);
  const installedIds = new Set((skills ?? []).map((skill) => skill.id));
  const availableSelectedCount = (skills ?? []).filter((skill) =>
    selected.has(skill.id),
  ).length;
  const unavailableSelectedCount = selectedIds.filter(
    (id) => !installedIds.has(id),
  ).length;
  const allVisibleSelected =
    skills !== null &&
    skills.length > 0 &&
    availableSelectedCount === skills.length;
  const selectAllChecked = selectAllMode || allVisibleSelected;
  const selectAllMixed =
    !selectAllChecked &&
    availableSelectedCount > 0 &&
    skills !== null &&
    availableSelectedCount < skills.length;

  useEffect(() => {
    if (selectAll.current) selectAll.current.indeterminate = selectAllMixed;
  }, [selectAllMixed]);

  const filteredSkills = (skills ?? []).filter((skill) =>
    `${skill.id} ${skill.name} ${skill.description}`
      .toLocaleLowerCase()
      .includes(query.trim().toLocaleLowerCase()),
  );

  function toggleSkill(id: string, checked: boolean) {
    const next = new Set(selectedIds);
    if (checked) next.add(id);
    else next.delete(id);
    setSelectedIds([...next]);
    setSelectAllMode(false);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      onCancel();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = Array.from(
      event.currentTarget.querySelectorAll<HTMLElement>(
        "button:not(:disabled), input:not(:disabled)",
      ),
    );
    const first = focusable[0];
    const last = focusable.at(-1);
    if (!first || !last) {
      event.preventDefault();
    } else if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  function apply() {
    if (skills === null || loading || error) return;
    const everyInstalledSkillSelected =
      skills.length > 0 && skills.every((skill) => selected.has(skill.id));
    onApply(
      selectAllMode || everyInstalledSkillSelected
        ? undefined
        : [...selected].sort(),
    );
  }

  return (
    <div
      className="dialog-backdrop skill-dialog-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
      onKeyDown={handleKeyDown}
    >
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby="skill-selector-heading"
        aria-describedby="skill-selector-description"
        className="confirm-dialog skill-dialog"
      >
        <div className="skill-dialog-heading">
          <div>
            <span className="eyebrow">NODE SETTINGS</span>
            <h2 id="skill-selector-heading">Select skills</h2>
          </div>
          <button
            className="icon-button"
            aria-label="Close skill selector"
            onClick={onCancel}
          >
            <X size={18} />
          </button>
        </div>
        <p id="skill-selector-description" className="skill-dialog-description">
          Choose skills for this node. Selection is saved with the workflow.
        </p>
        <div className="skill-dialog-toolbar">
          <label className="skill-select-all">
            <input
              ref={selectAll}
              type="checkbox"
              checked={selectAllChecked}
              disabled={loading || error !== "" || skills === null}
              onChange={(event) => {
                const checked = event.target.checked;
                setSelectAllMode(checked);
                setSelectedIds(
                  checked ? (skills ?? []).map((skill) => skill.id) : [],
                );
              }}
            />
            <span>Select all</span>
          </label>
          <input
            ref={search}
            className="skill-search"
            aria-label="Search installed skills"
            placeholder="Search skills"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>
        <div className="skill-dialog-list" role="group" aria-label="Installed skills">
          {loading && <p role="status">Loading installed skills…</p>}
          {!loading && error && (
            <div className="skill-dialog-error" role="alert">
              <p>{error}</p>
              <button className="button button-secondary" onClick={onRetry}>
                Retry
              </button>
            </div>
          )}
          {!loading && !error && skills?.length === 0 && (
            <p className="skill-dialog-empty">
              No installed skills found on this device.
            </p>
          )}
          {!loading && !error && skills && skills.length > 0 &&
            (filteredSkills.length ? (
              filteredSkills.map((skill) => (
                <label className="skill-option" key={skill.id}>
                  <input
                    type="checkbox"
                    checked={selected.has(skill.id)}
                    onChange={(event) =>
                      toggleSkill(skill.id, event.target.checked)
                    }
                  />
                  <span className="skill-option-copy">
                    <strong>{skill.id}</strong>
                    {skill.description && <small>{skill.description}</small>}
                  </span>
                </label>
              ))
            ) : (
              <p className="skill-dialog-empty">No skills match this search.</p>
            ))}
        </div>
        <div className="skill-dialog-footer">
          <span>
            {loading
              ? "Loading…"
              : `${availableSelectedCount} of ${skills?.length ?? 0} installed skills selected`}
            {unavailableSelectedCount > 0 &&
              ` · ${unavailableSelectedCount} saved skills are not installed`}
          </span>
          <div>
            <button className="button button-secondary" onClick={onCancel}>
              Cancel
            </button>
            <button
              className="button button-primary"
              disabled={skills === null || loading || error !== ""}
              onClick={apply}
            >
              Apply
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
