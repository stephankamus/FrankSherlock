import { useState, useCallback, useRef } from "react";

type DragProps = {
  draggable: boolean;
  onDragStart: (e: React.DragEvent) => void;
  onDragOver: (e: React.DragEvent) => void;
  onDrop: (e: React.DragEvent) => void;
  onDragEnd: () => void;
  className: string;
};

type UseDragReorderOptions<T extends { id: number }> = {
  items: T[];
  onReorder: (ids: number[]) => void;
  readOnly?: boolean;
};

export function useDragReorder<T extends { id: number }>({
  items,
  onReorder,
  readOnly,
}: UseDragReorderOptions<T>) {
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const dragMimeType = useRef(`application/x-reorder-${Math.random().toString(36).slice(2)}`);

  const getDragProps = useCallback(
    (index: number): DragProps => {
      if (readOnly) {
        return {
          draggable: false,
          onDragStart: () => {},
          onDragOver: () => {},
          onDrop: () => {},
          onDragEnd: () => {},
          className: "",
        };
      }

      return {
        draggable: true,
        onDragStart: (e: React.DragEvent) => {
          e.dataTransfer.effectAllowed = "move";
          e.dataTransfer.setData(dragMimeType.current, String(index));
          setDragIndex(index);
        },
        onDragOver: (e: React.DragEvent) => {
          if (dragIndex === null) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = "move";
          setDropIndex(index);
        },
        onDrop: (e: React.DragEvent) => {
          e.preventDefault();
          const fromStr = e.dataTransfer.getData(dragMimeType.current);
          if (!fromStr) return;
          const from = parseInt(fromStr, 10);
          if (isNaN(from) || from === index) {
            setDragIndex(null);
            setDropIndex(null);
            return;
          }
          const newItems = [...items];
          const [moved] = newItems.splice(from, 1);
          newItems.splice(index, 0, moved);
          onReorder(newItems.map((item) => item.id));
          setDragIndex(null);
          setDropIndex(null);
        },
        onDragEnd: () => {
          setDragIndex(null);
          setDropIndex(null);
        },
        className: [
          dragIndex === index ? "dragging" : "",
          dropIndex === index && dragIndex !== index ? "drag-over" : "",
        ]
          .filter(Boolean)
          .join(" "),
      };
    },
    [items, onReorder, readOnly, dragIndex, dropIndex],
  );

  return { getDragProps };
}
