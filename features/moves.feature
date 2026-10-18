Feature: Moves in v1 are a delete plus a copy
  Decision 20: a move deletes the original and inserts a copy in one
  transaction. These scenarios pin its known gaps as expected failures.

  @vector:move-with-concurrent-edit
  Scenario: A concurrent edit to a moved node is lost (known gap)
    Given a group and a rectangle
    When Ann moves the rectangle into the group and Ben recolours it at the same time
    Then the rectangle is in the group, but Ben's colour is lost

  @vector:concurrent-moves-same-node
  Scenario: Two concurrent moves of one node duplicate it (known gap)
    Given two groups and a rectangle
    When Ann moves the rectangle into the first group and Ben into the second at the same time
    Then both groups hold a copy of the rectangle
