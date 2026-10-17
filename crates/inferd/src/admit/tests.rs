use super::*;

const A: Lane = Lane(0);
const B: Lane = Lane(1);

fn one_lane(seats: u32) -> Admission {
    let mut admission = Admission::default();
    admission.set_lane(A, Seats::of(seats));
    admission
}

fn waiting(ahead: u32) -> Place {
    Place::Waiting {
        ahead: Count(ahead),
    }
}

#[test]
fn a_lone_person_session_never_waits() {
    let mut admission = one_lane(1);
    let first = admission.arrive(&[A], Priority::Person).expect("lane");
    assert_eq!(first.place, Place::Seated);
    admission.leave(first.ticket);
    let second = admission.arrive(&[A], Priority::Person).expect("lane");
    assert_eq!(second.place, Place::Seated);
}

#[test]
fn the_person_goes_ahead_of_every_helper_and_helpers_keep_their_order() {
    let mut admission = one_lane(1);
    let busy = admission.arrive(&[A], Priority::Helper).expect("lane");
    let first = admission.arrive(&[A], Priority::Helper).expect("lane");
    let second = admission.arrive(&[A], Priority::Helper).expect("lane");
    assert_eq!((first.place, second.place), (waiting(0), waiting(1)));
    // The person arrives last and stands first.
    let person = admission.arrive(&[A], Priority::Person).expect("lane");
    assert_eq!(person.place, waiting(0));
    assert_eq!(admission.ahead(first.ticket), Some(Count(1)));
    assert_eq!(admission.ahead(second.ticket), Some(Count(2)));
    // The order the seat is handed on: person, then the helpers as they asked.
    let mut served = Vec::new();
    let mut current = busy.ticket;
    for _ in 0..3 {
        let promoted = admission.leave(current);
        assert_eq!(promoted.len(), 1);
        current = promoted[0].ticket;
        served.push(current);
    }
    assert_eq!(served, [person.ticket, first.ticket, second.ticket]);
}

#[test]
fn a_person_waits_behind_persons_only() {
    let mut admission = one_lane(1);
    admission.arrive(&[A], Priority::Helper).expect("lane");
    admission.arrive(&[A], Priority::Helper).expect("lane");
    let first = admission.arrive(&[A], Priority::Person).expect("lane");
    let second = admission.arrive(&[A], Priority::Person).expect("lane");
    assert_eq!((first.place, second.place), (waiting(0), waiting(1)));
}

#[test]
fn seats_are_as_many_as_the_engine_takes() {
    let mut admission = one_lane(2);
    let places: Vec<Place> = (0..3)
        .map(|_| {
            admission
                .arrive(&[A], Priority::Helper)
                .expect("lane")
                .place
        })
        .collect();
    assert_eq!(places, [Place::Seated, Place::Seated, waiting(0)]);
    // An engine whose seats are unknown has one.
    assert_eq!(Seats::of(0), Seats::of(1));
    // More seats seat what waits.
    assert_eq!(admission.set_lane(A, Seats::of(3)).len(), 1);
}

#[test]
fn two_instances_spread_requests_then_the_shortest_queue_wins() {
    let mut admission = Admission::default();
    admission.set_lane(A, Seats::of(1));
    admission.set_lane(B, Seats::of(1));
    let lanes = [A, B];
    let first = admission.arrive(&lanes, Priority::Helper).expect("lane");
    let second = admission.arrive(&lanes, Priority::Helper).expect("lane");
    assert_eq!(
        (first.lane, second.lane, second.place),
        (A, B, Place::Seated)
    );
    // Both full: the queues grow evenly, the lower lane first on a tie.
    let third = admission.arrive(&lanes, Priority::Helper).expect("lane");
    let fourth = admission.arrive(&lanes, Priority::Helper).expect("lane");
    assert_eq!((third.lane, fourth.lane), (A, B));
    // A lane with a free seat beats a shorter queue elsewhere.
    admission.leave(second.ticket);
    admission.leave(fourth.ticket);
    let fifth = admission.arrive(&lanes, Priority::Helper).expect("lane");
    assert_eq!((fifth.lane, fifth.place), (B, Place::Seated));
}

#[test]
fn a_person_picks_the_lane_where_fewer_persons_wait() {
    let mut admission = Admission::default();
    admission.set_lane(A, Seats::of(1));
    admission.set_lane(B, Seats::of(1));
    let lanes = [A, B];
    admission.arrive(&[A], Priority::Helper).expect("lane");
    admission.arrive(&[B], Priority::Helper).expect("lane");
    // A has two helpers waiting, B has none, but the person stands ahead of every helper, so
    // neither queue is longer for them: the tie goes to the lower lane.
    admission.arrive(&[A], Priority::Helper).expect("lane");
    admission.arrive(&[A], Priority::Helper).expect("lane");
    let person = admission.arrive(&lanes, Priority::Person).expect("lane");
    assert_eq!((person.lane, person.place), (A, waiting(0)));
}

#[test]
fn a_seat_is_freed_when_a_request_ends_is_cancelled_or_fails() {
    // The three look the same to admission: the request leaves, seated or waiting.
    let mut admission = one_lane(1);
    let running = admission.arrive(&[A], Priority::Helper).expect("lane");
    let cancelled = admission.arrive(&[A], Priority::Helper).expect("lane");
    let next = admission.arrive(&[A], Priority::Helper).expect("lane");
    assert!(admission.leave(cancelled.ticket).is_empty());
    assert_eq!(admission.ahead(next.ticket), Some(Count(0)));
    let promoted = admission.leave(running.ticket);
    assert_eq!(
        promoted,
        [Promoted {
            ticket: next.ticket,
            lane: A
        }]
    );
    admission.leave(next.ticket);
    assert_eq!(admission.load(A), Count(0));
    // Leaving twice is harmless.
    assert!(admission.leave(next.ticket).is_empty());
}

#[test]
fn a_lost_lane_hands_back_what_it_held_and_the_waiting_move_on() {
    let mut admission = Admission::default();
    admission.set_lane(A, Seats::of(1));
    admission.set_lane(B, Seats::of(1));
    let running = admission.arrive(&[A], Priority::Person).expect("lane");
    let waits = admission.arrive(&[A], Priority::Helper).expect("lane");
    let orphans = admission.drop_lane(A);
    assert_eq!(
        orphans,
        Orphans {
            seated: vec![(running.ticket, Priority::Person)],
            waiting: vec![(waits.ticket, Priority::Helper)],
        }
    );
    let moved = admission
        .rehome(waits.ticket, Priority::Helper, &[B])
        .expect("lane");
    assert_eq!((moved.lane, moved.place), (B, Place::Seated));
    // Nothing is left of the lane.
    assert_eq!(admission.arrive(&[A], Priority::Person), None);
}
