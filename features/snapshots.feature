Feature: Snapshots name points in history

  @vector:delete-changes-snapshot
  Scenario: Deleting a shape changes the snapshot hash
    Given a drawing with two shapes
    When Ann deletes one
    Then the snapshot hash changes, though nothing was inserted

  @vector:fork-keeps-later-deletes
  Scenario: A fork rebuilt from its snapshot keeps content deleted later
    Given Ann recoloured one of two rectangles
    When Ben then deletes the other
    Then a document rebuilt from the snapshot before the delete still has both
